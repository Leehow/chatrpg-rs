//! 实体桥接边:两场景共享 referenced 实体即连边(Gorinski&Lapata,零 LLM)。纯、确定、可单测。
use super::units::Unit;
use trpg_model::{LinkType, ScenarioLink, ScenarioNode};
use std::collections::HashSet;

/// TOC 局部窗口半径:骨架顺序 ±WINDOW 内的场景视为可能衔接。
const NEIGHBOR_WINDOW: usize = 3;

/// 收集一个场景引用的全部实体 id(npc/clue/location/encounter)为去重集合。
fn ent_set<'a>(n: &'a ScenarioNode) -> HashSet<&'a str> {
    n.referenced_npc_ids.iter().chain(&n.referenced_clue_ids)
        .chain(&n.referenced_location_ids).chain(&n.referenced_encounter_ids)
        .map(|s| s.as_str()).collect()
}

/// 两场景共享 ≥1 referenced 实体(npc/clue/location/encounter)→ 候选边 (i, j, shared_count)。
pub fn bridge_edges(scenes: &[ScenarioNode]) -> Vec<(usize, usize, usize)> {
    let sets: Vec<HashSet<&str>> = scenes.iter().map(ent_set).collect();
    let mut out = Vec::new();
    for i in 0..scenes.len() {
        for j in (i + 1)..scenes.len() {
            let shared = sets[i].intersection(&sets[j]).count();
            if shared > 0 { out.push((i, j, shared)); }
        }
    }
    out
}

/// 把桥接边就地填进双方 links(双向;按 to_node_id 去重,不覆盖已有边)。返回新增边数。
pub fn apply_bridge_edges(scenes: &mut Vec<ScenarioNode>) -> usize {
    let edges = bridge_edges(scenes);
    let ids: Vec<String> = scenes.iter().map(|s| s.node_id.clone()).collect();
    let mut added = 0;
    for (i, j, shared) in edges {
        for (a, b) in [(i, j), (j, i)] {
            let to = ids[b].clone();
            if scenes[a].links.iter().any(|l| l.to_node_id == to) { continue; }
            scenes[a].links.push(ScenarioLink {
                to_node_id: to, reason: format!("共享 {shared} 个实体"),
                clue_id: None, link_type: LinkType::Spatial, source_anchor: None,
            });
            added += 1;
        }
    }
    added
}

/// 给场景 idx 找"候选衔接场景"node_id 列表(供深抽时让 LLM 从中选实际出口)。
/// = TOC 局部(骨架顺序相邻 ±WINDOW)∪ 实体共享者(复用 bridge_edges 中含 idx 的对)。
/// 去重、排除自身、保持稳定顺序(先 TOC 局部、再实体共享)。纯、确定。
pub fn candidate_neighbors(scenes: &[ScenarioNode], idx: usize) -> Vec<String> {
    if idx >= scenes.len() {
        return Vec::new();
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let push = |id: &str, seen: &mut HashSet<String>, out: &mut Vec<String>| {
        if id.is_empty() { return; }
        if seen.insert(id.to_string()) {
            out.push(id.to_string());
        }
    };
    // TOC 局部窗口 [idx-W, idx+W],边界裁剪,排除自身。
    let lo = idx.saturating_sub(NEIGHBOR_WINDOW);
    let hi = (idx + NEIGHBOR_WINDOW).min(scenes.len() - 1);
    for j in lo..=hi {
        if j == idx { continue; }
        push(&scenes[j].node_id, &mut seen, &mut out);
    }
    // 实体共享者:bridge_edges 中含 idx 的对的另一端。
    for (i, j, _shared) in bridge_edges(scenes) {
        let other = if i == idx { Some(j) } else if j == idx { Some(i) } else { None };
        if let Some(o) = other {
            push(&scenes[o].node_id, &mut seen, &mut out);
        }
    }
    out
}

/// 给没有 page_start 的场景,用 units 的 page_numbers 兜底解析页码(场景标题匹配 unit 标题/heading)。
/// 确定性:sanitize_key 归一后,scene.title 的 key 与某 unit 的 title/heading key 相等或含子串关系 →
/// 取该 unit page_numbers 的最小/最大设为 scene.page_start/page_end。匹配不到 → 保持 None
/// (fail-closed,不强造)。返回成功解析的场景数。纯、确定。
pub fn resolve_scene_pages(scenes: &mut [ScenarioNode], units: &[Unit]) -> usize {
    let mut resolved = 0;
    for s in scenes.iter_mut() {
        if s.page_start.is_some() {
            continue;
        }
        let skey = sanitize_key(&s.title);
        if skey.is_empty() {
            continue;
        }
        // 在 units 里找标题/heading key 与场景 key 相等或互含的、且带页码的 unit。
        let mut best: Option<(u32, u32)> = None;
        for u in units {
            if u.page_numbers.is_empty() {
                continue;
            }
            if !unit_title_keys_match(u, &skey) {
                continue;
            }
            let lo = u.page_numbers.iter().copied().min().unwrap();
            let hi = u.page_numbers.iter().copied().max().unwrap();
            best = match best {
                Some((blo, bhi)) => Some((blo.min(lo), bhi.max(hi))),
                None => Some((lo, hi)),
            };
        }
        if let Some((lo, hi)) = best {
            s.page_start = Some(lo);
            s.page_end = Some(hi);
            resolved += 1;
        }
    }
    resolved
}

/// unit 的 title 或任一 heading_context 的归一 key 与场景 key 相等或互为子串。
fn unit_title_keys_match(u: &Unit, scene_key: &str) -> bool {
    let cand = std::iter::once(u.title.as_str()).chain(u.heading_context.iter().map(|s| s.as_str()));
    for c in cand {
        let ck = sanitize_key(c);
        if ck.is_empty() {
            continue;
        }
        if ck == scene_key || ck.contains(scene_key) || scene_key.contains(&ck) {
            return true;
        }
    }
    false
}

/// 实体名/id 归一 key:小写,仅保留字母数字(含 CJK)。
pub fn sanitize_key(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 按 name 的 sanitize_key 精确合并重复实体(保留首个为规范),并把所有场景 referenced_npc_ids
/// 里指向被合并 id 的引用重写到规范 id。返回合并掉的实体数。纯、确定。
/// (模糊对的 LLM 二次确认是 reader 集成步,不在此纯函数。)
pub fn dedup_entities_by_key(entities: &mut Vec<serde_json::Value>, scenes: &mut [ScenarioNode]) -> usize {
    use std::collections::HashMap;
    let mut canon: HashMap<String, String> = HashMap::new(); // key -> 规范 id
    let mut remap: HashMap<String, String> = HashMap::new();  // 旧 id -> 规范 id
    let mut keep = Vec::new();
    let mut merged = 0;
    for e in entities.drain(..) {
        let id = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name = e.get("name").and_then(|v| v.as_str()).unwrap_or(&id);
        let k = sanitize_key(name);
        if k.is_empty() { keep.push(e); continue; }
        if let Some(cid) = canon.get(&k) {
            remap.insert(id, cid.clone());
            merged += 1;
        } else {
            canon.insert(k, id.clone());
            keep.push(e);
        }
    }
    *entities = keep;
    if !remap.is_empty() {
        let rewrite = |v: &mut Vec<String>| { for x in v.iter_mut() { if let Some(c) = remap.get(x) { *x = c.clone(); } } };
        for s in scenes.iter_mut() {
            rewrite(&mut s.referenced_npc_ids);
            rewrite(&mut s.referenced_clue_ids);
            rewrite(&mut s.referenced_location_ids);
            rewrite(&mut s.referenced_encounter_ids);
        }
    }
    merged
}

/// 兜底入口选择（确定性）：首个 node_type=="scene"/"story" 的场景；无则首个。空 → None。
/// 仅在 reader 没给出语义入口时使用，保留 reader 提交的顺序。
/// （从 module_reader.rs 外移守 ≤400 行；经其 `pub(super) use` 再导出，调用点/测试不变。）
pub(super) fn entry_scene_index(scenes: &[ScenarioNode]) -> Option<usize> {
    if scenes.is_empty() {
        return None;
    }
    scenes
        .iter()
        .position(|n| matches!(n.node_type.as_str(), "scene" | "story"))
        .or(Some(0))
}

/// 选取 Pass B 的入口场景索引。**语义优先**：先用 reader 自己判定的 entry_node_id
/// （它读懂了这本模组、会跳过前言/安全提示/目录等非可玩前置）；reader 未给或 id 失效
/// → 退到确定性 `entry_scene_index`。主路径不靠 node_type 字面关键词匹配，符合语义优先理念。
pub(super) fn resolve_entry_index(scenes: &[ScenarioNode], entry_node_id: Option<&str>) -> Option<usize> {
    if let Some(id) = entry_node_id.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(i) = scenes.iter().position(|n| n.node_id == id) {
            return Some(i);
        }
    }
    entry_scene_index(scenes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene(id: &str, npcs: &[&str]) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.referenced_npc_ids = npcs.iter().map(|s| s.to_string()).collect();
        n
    }
    #[test]
    fn shared_entity_makes_edge_none_otherwise() {
        let scenes = vec![scene("a", &["npc1"]), scene("b", &["npc1"]), scene("c", &["npc2"])];
        let e = bridge_edges(&scenes);
        assert_eq!(e, vec![(0, 1, 1)], "a,b 共享 npc1 → 一条边;c 无共享");
    }
    #[test]
    fn apply_is_bidirectional_and_dedups() {
        let mut scenes = vec![scene("a", &["npc1"]), scene("b", &["npc1"])];
        let n1 = apply_bridge_edges(&mut scenes);
        assert_eq!(n1, 2, "双向各一条");
        assert_eq!(scenes[0].links[0].to_node_id, "b");
        assert_eq!(scenes[0].links[0].link_type, LinkType::Spatial);
        let n2 = apply_bridge_edges(&mut scenes);
        assert_eq!(n2, 0, "重复调用不重复加(去重)");
    }
    #[test]
    fn sanitize_key_normalizes() {
        assert_eq!(sanitize_key("拉斯 (Russell)!"), sanitize_key("拉斯russell"));
        assert_eq!(sanitize_key("Dr. Brenner"), "drbrenner");
    }
    fn scene_titled(id: &str, title: &str) -> ScenarioNode {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.title = title.into();
        n
    }
    fn unit(title: &str, pages: &[u32], headings: &[&str]) -> Unit {
        Unit {
            title: title.into(),
            page_numbers: pages.to_vec(),
            heading_context: headings.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }
    #[test]
    fn candidate_neighbors_includes_toc_local() {
        let scenes = vec![scene("a", &[]), scene("b", &[]), scene("c", &[]),
                          scene("d", &[]), scene("e", &[])];
        let n = candidate_neighbors(&scenes, 2);
        // W=3 → 全部 a,b,d,e(裁剪到边界),自身 c 排除。
        assert!(n.contains(&"a".to_string()) && n.contains(&"b".to_string()));
        assert!(n.contains(&"d".to_string()) && n.contains(&"e".to_string()));
        assert!(!n.contains(&"c".to_string()), "排除自身");
    }
    #[test]
    fn candidate_neighbors_includes_distant_entity_sharer() {
        // 远处场景(超出 ±3 窗口)但共享实体 → 仍进候选。
        let scenes = vec![
            scene("s0", &["npc1"]), scene("s1", &[]), scene("s2", &[]),
            scene("s3", &[]), scene("s4", &[]), scene("s5", &[]),
            scene("s6", &["npc1"]),
        ];
        let n = candidate_neighbors(&scenes, 0);
        assert!(n.contains(&"s6".to_string()), "远处共享实体者也进候选");
    }
    #[test]
    fn candidate_neighbors_excludes_self_and_dedups() {
        // s0 既在 s1 的 TOC 窗口内,又与 s1 共享实体 → 只出现一次,且不含自身。
        let scenes = vec![scene("s0", &["npc1"]), scene("s1", &["npc1"]), scene("s2", &[])];
        let n = candidate_neighbors(&scenes, 1);
        assert!(!n.contains(&"s1".to_string()), "排除自身");
        let count_s0 = n.iter().filter(|x| *x == "s0").count();
        assert_eq!(count_s0, 1, "TOC 局部 + 实体共享不重复");
    }
    #[test]
    fn resolve_scene_pages_fills_from_matching_unit() {
        let mut scenes = vec![scene_titled("n1", "拉斯的农场 (Russell's Farm)")];
        let units = vec![
            unit("拉斯的农场 Russell's Farm", &[12, 13, 14], &[]),
            unit("无关章节", &[99], &[]),
        ];
        let resolved = resolve_scene_pages(&mut scenes, &units);
        assert_eq!(resolved, 1);
        assert_eq!(scenes[0].page_start, Some(12));
        assert_eq!(scenes[0].page_end, Some(14));
    }
    #[test]
    fn resolve_scene_pages_matches_via_heading_context() {
        let mut scenes = vec![scene_titled("n1", "The Vault")];
        let units = vec![unit("intro para", &[7, 8], &["Chapter 2", "The Vault"])];
        let resolved = resolve_scene_pages(&mut scenes, &units);
        assert_eq!(resolved, 1);
        assert_eq!(scenes[0].page_start, Some(7));
        assert_eq!(scenes[0].page_end, Some(8));
    }
    #[test]
    fn resolve_scene_pages_failclosed_on_no_match() {
        let mut scenes = vec![scene_titled("n1", "Homecoming")];
        let units = vec![unit("Totally Different", &[42], &[])];
        let resolved = resolve_scene_pages(&mut scenes, &units);
        assert_eq!(resolved, 0, "标题不匹配 → 不解析");
        assert_eq!(scenes[0].page_start, None, "fail-closed 保持 None");
        assert_eq!(scenes[0].page_end, None);
    }
    #[test]
    fn resolve_scene_pages_skips_already_paged() {
        let mut scenes = vec![scene_titled("n1", "拉斯的农场")];
        scenes[0].page_start = Some(5);
        let units = vec![unit("拉斯的农场", &[12, 13], &[])];
        let resolved = resolve_scene_pages(&mut scenes, &units);
        assert_eq!(resolved, 0, "已有 page_start 的场景跳过");
        assert_eq!(scenes[0].page_start, Some(5), "不覆盖既有页码");
    }
    #[test]
    fn dedup_merges_by_key_and_rewrites_refs() {
        let mut npcs = vec![
            serde_json::json!({"id":"npc_brenner","name":"Dr. Brenner"}),
            serde_json::json!({"id":"npc_brenner2","name":"Dr Brenner"}),
        ];
        let mut scenes = vec![{ let mut n=ScenarioNode::default(); n.node_id="s1".into();
            n.referenced_npc_ids=vec!["npc_brenner2".into()]; n }];
        let merged = dedup_entities_by_key(&mut npcs, &mut scenes);
        assert_eq!(merged, 1, "两个 Brenner 归一");
        assert_eq!(npcs.len(), 1);
        // 场景引用被重写到规范 id(保留的那个)
        let canon = npcs[0]["id"].as_str().unwrap().to_string();
        assert_eq!(scenes[0].referenced_npc_ids, vec![canon]);
    }
}
