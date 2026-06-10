//! module_reader 的单元测试（从 module_reader.rs 物理拆出守 ≤400 行）。
//! 经 `#[cfg(test)] #[path = "module_reader_tests.rs"] mod tests;` 引入，
//! 仍是 module_reader 的子模块：`use super::*` + 私有 fn 可见性不变。
//! Tests cover deterministic parts only; the LLM loop is validated live in Phase 6.
use super::*;
use trpg_model::{LinkType, ScenarioLink};

#[test]
fn closure_collects_refs_and_links_dedup() {
    let mut n = ScenarioNode::default();
    n.referenced_npc_ids = vec!["a".into(), "b".into()];
    n.referenced_clue_ids = vec!["b".into()]; // dup
    n.links = vec![ScenarioLink {
        to_node_id: "c".into(),
        reason: "".into(),
        clue_id: None,
        link_type: LinkType::Spatial,
        source_anchor: None,
    }];
    let c = dependency_closure(&n);
    assert_eq!(c, vec!["a", "b", "c"]);
}

#[test]
fn stub_to_node_is_fail_closed_skeleton() {
    let v = json!({"node_id":"loc1","title":"加油站","kind":"location","page_start":17});
    let n = stub_to_node(&v).unwrap();
    assert_eq!(n.extraction_status, SceneExtractionStatus::SkeletonOnly);
    assert!(n.read_aloud.is_none(), "绝不编造 read_aloud");
    assert_eq!(n.page_start, Some(17));
    assert!(stub_to_node(&json!({"title":"无id"})).is_none());
}

#[test]
fn entry_scene_prefers_story_then_first() {
    let mk = |id: &str, kind: &str| {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.node_type = kind.into();
        n
    };
    let scenes = vec![mk("a", "location"), mk("b", "scene"), mk("c", "story")];
    assert_eq!(entry_scene_index(&scenes), Some(1));
    let only_loc = vec![mk("a", "location")];
    assert_eq!(entry_scene_index(&only_loc), Some(0));
    assert_eq!(entry_scene_index(&[]), None);
}

#[test]
fn resolve_entry_prefers_reader_id_then_heuristic() {
    let mk = |id: &str, kind: &str| {
        let mut n = ScenarioNode::default();
        n.node_id = id.into();
        n.node_type = kind.into();
        n
    };
    // 卷首语在前、序幕(scene)在中、地点在后 —— 复现"前言陷阱"场景
    let scenes = vec![mk("preface", "guidance"), mk("prologue", "scene"), mk("loc1", "location")];
    // 语义优先：reader 指定 loc1（即使它不是 scene/story 类型）也采纳
    assert_eq!(resolve_entry_index(&scenes, Some("loc1")), Some(2), "reader 语义入口即采纳");
    // 失效 id → 兜底 entry_scene_index（首个 scene/story = prologue@1），而非取 preface@0
    assert_eq!(resolve_entry_index(&scenes, Some("missing")), Some(1), "无效 id → 确定性兜底跳过前言");
    // 缺失 → 兜底
    assert_eq!(resolve_entry_index(&scenes, None), Some(1));
    assert_eq!(resolve_entry_index(&[], Some("x")), None, "空 → None");
}

#[test]
fn apply_deep_keeps_read_aloud_none_when_empty() {
    let mut n = ScenarioNode::default();
    n.node_id = "loc1".into();
    apply_deep_to_node(&mut n, &json!({"scene":{"read_aloud":"  ","gm_notes":""}}));
    assert!(n.read_aloud.is_none(), "空白念白 → 保持 None，绝不编造");
    assert_eq!(n.extraction_status, SceneExtractionStatus::SkeletonOnly, "无正文 → 不翻 Deep");
}

#[test]
fn apply_deep_flips_to_deep_extracted_with_content() {
    let mut n = ScenarioNode::default();
    n.node_id = "loc1".into();
    apply_deep_to_node(
        &mut n,
        &json!({"scene":{
            "read_aloud":"你们看到一个褪色的广告牌。",
            "referenced_npc_ids":["npc_a"],
            "links":[{"to_node_id":"loc2","reason":"可达","link_type":"spatial"}]
        }}),
    );
    assert_eq!(n.extraction_status, SceneExtractionStatus::DeepExtracted);
    assert_eq!(n.referenced_npc_ids, vec!["npc_a"]);
    assert_eq!(n.links[0].link_type, LinkType::Spatial);
}

#[test]
fn apply_deep_links_scene_to_entities_from_entities_array() {
    // LLM 把实体放进 deep.entities 但 scene.referenced_* 留空 → 应从 entities 按 kind 补链
    let mut n = ScenarioNode::default();
    n.node_id = "sc01".into();
    apply_deep_to_node(
        &mut n,
        &json!({
            "scene": {"gm_notes": "拉斯在加油站等候。"},
            "entities": [
                {"id": "npc1", "kind": "npc", "name": "拉斯"},
                {"id": "loc1", "kind": "location", "name": "加油站"},
                {"id": "", "kind": "npc", "name": "无id丢弃"}
            ]
        }),
    );
    assert!(n.referenced_npc_ids.contains(&"npc1".to_string()), "entities 的 npc 应补进 referenced_npc_ids");
    assert!(n.referenced_location_ids.contains(&"loc1".to_string()), "entities 的 location 应补进 referenced_location_ids");
    assert_eq!(n.referenced_npc_ids.len(), 1, "空 id 应跳过、不重复");
}

#[test]
fn apply_deep_entities_supplement_dedups_against_scene_refs() {
    // scene.referenced_* 已有的 id + entities 重复 id → 合并去重、不覆盖既有
    let mut n = ScenarioNode::default();
    n.node_id = "sc01".into();
    apply_deep_to_node(
        &mut n,
        &json!({
            "scene": {"gm_notes": "x", "referenced_npc_ids": ["npc1"]},
            "entities": [
                {"id": "npc1", "kind": "npc"},
                {"id": "npc2", "kind": "npc"}
            ]
        }),
    );
    assert_eq!(n.referenced_npc_ids, vec!["npc1", "npc2"], "保留 scene 已填、追加 entities 新 id、去重");
}

#[test]
fn deep_empty_links_preserves_skeleton_links() {
    // 回归：深抽因 source_anchor 严格过滤交了空 links → 绝不冲掉骨架 Pass A 的导航边
    let mut n = ScenarioNode::default();
    n.node_id = "sc01".into();
    n.links = vec![ScenarioLink {
        to_node_id: "sc02".into(),
        reason: "骨架边".into(),
        clue_id: None,
        link_type: LinkType::Spatial,
        source_anchor: None,
    }];
    apply_deep_to_node(&mut n, &json!({"scene":{"gm_notes":"正文","links":[]}}));
    assert_eq!(n.links.len(), 1, "深抽空 links 不得冲掉骨架边");
    assert_eq!(n.links[0].to_node_id, "sc02");
}

#[test]
fn deep_links_merge_supplement_and_upgrade() {
    // 深抽新 target 追加；同 target 带 source_anchor → 升级覆盖骨架那条
    let mut n = ScenarioNode::default();
    n.node_id = "sc01".into();
    n.links = vec![ScenarioLink {
        to_node_id: "sc02".into(),
        reason: "骨架".into(),
        clue_id: None,
        link_type: LinkType::Spatial,
        source_anchor: None,
    }];
    apply_deep_to_node(
        &mut n,
        &json!({"scene":{"gm_notes":"正文","links":[
            {"to_node_id":"sc02","reason":"深抽精确","link_type":"spatial","source_anchor":"穿过北门"},
            {"to_node_id":"sc03","reason":"新出口","link_type":"trigger"}
        ]}}),
    );
    assert_eq!(n.links.len(), 2, "sc02 去重升级 + sc03 追加");
    let sc02 = n.links.iter().find(|l| l.to_node_id == "sc02").unwrap();
    assert_eq!(sc02.source_anchor.as_deref(), Some("穿过北门"), "同 target 深抽带锚优先");
    assert!(n.links.iter().any(|l| l.to_node_id == "sc03"), "深抽新 target 追加");
}

#[test]
fn ensure_entry_connected_patches_isolated_entry() {
    // 入口无出边 → 补一条到下一场景的 Sequential 兜底边
    let mut a = ScenarioNode::default();
    a.node_id = "entry".into();
    let mut b = ScenarioNode::default();
    b.node_id = "hub".into();
    let mut scenes = vec![a, b];
    let patched = ensure_entry_connected(&mut scenes, 0);
    assert!(patched, "孤立入口应被补边");
    assert_eq!(scenes[0].links.len(), 1);
    assert_eq!(scenes[0].links[0].to_node_id, "hub");
    assert_eq!(scenes[0].links[0].link_type, LinkType::Sequential);
}

#[test]
fn ensure_entry_connected_noop_when_entry_has_links() {
    // 入口已有出边 → 不动
    let mut a = ScenarioNode::default();
    a.node_id = "entry".into();
    a.links = vec![ScenarioLink {
        to_node_id: "x".into(),
        reason: "".into(),
        clue_id: None,
        link_type: LinkType::Spatial,
        source_anchor: None,
    }];
    let b = ScenarioNode::default();
    let mut scenes = vec![a, b];
    assert!(!ensure_entry_connected(&mut scenes, 0), "已有出边不补");
    assert_eq!(scenes[0].links.len(), 1);
}

#[test]
fn merge_deep_entities_routes_by_kind_and_upserts() {
    let mut out = ModuleReadout::default();
    out.npcs = vec![json!({"id":"npc_a","name":"旧"})]; // skeleton stub
    merge_deep_entities(
        &mut out,
        &json!({"entities":[
            {"id":"npc_a","kind":"npc","name":"新","gm_notes":"详情"},
            {"id":"clue_1","kind":"clue","name":"线索"},
            {"id":"","kind":"npc","name":"无id丢弃"}
        ]}),
    );
    assert_eq!(out.npcs.len(), 1, "同 id 应 upsert 不重复");
    assert_eq!(out.npcs[0].get("name").unwrap(), "新", "深抽详情覆盖骨架");
    assert_eq!(out.clues.len(), 1);
}
