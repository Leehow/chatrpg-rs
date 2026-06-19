//! 反剧透投影裁剪（ENFORCEMENT 半）：在场景/实体内容投进 GM context **之前**，
//! 对**未揭示**的剧透做外科裁剪。纯函数 + 单点接入 SceneNeedResolver，`scene_node_to_blocks`
//! 本体零改动（其字节回归测试不受影响）。
//!
//! 规则（守理念：别太严，绝不裁可玩内容）：
//! - 只对**显式标了 spoiler 且 fact_id 不在 revealed 账本**的实体/场景生效；
//!   没标剧透或已揭示 → **原样透传**（实体 JSON 字节不变）。
//! - 裁剪 = 把 secret_terms 的出现替换成占位符（保留可读性），并把显示名换成 public_alias；
//!   reveal_conditions 作 GM-only 提示注入（自身也过裁剪，防条件文案泄密）。
//! - 引擎**绝不**用关键词匹配自动判定 reveal_conditions 是否满足——揭示只由
//!   revealed-facts 账本显式落账驱动（零规则集硬编码）。
//! - fail-closed：调用方若取不到 revealed 集（DB 抖动）→ 传空集 = 全部按未揭示裁剪
//!   （宁可不泄，不赌 DB）。

use std::collections::HashSet;

use serde_json::{json, Value};
use trpg_model::{ScenarioNode, SpoilerMeta};

/// 纯裁剪：对未揭示的场景级 + 实体级剧透做替换/换名，返回可直接喂 `scene_node_to_blocks`
/// 的 (guarded_node, guarded_npcs)。`revealed` = 本会话已揭示 fact_id 集（entity_id/node_id）。
pub(crate) fn guard_scene(
    node: &ScenarioNode,
    npcs: &[Value],
    revealed: &HashSet<String>,
) -> (ScenarioNode, Vec<Value>) {
    let guarded_node = guard_node_text(node, revealed);
    let guarded_npcs = npcs.iter().map(|v| guard_entity(v, revealed)).collect();
    (guarded_node, guarded_npcs)
}

/// 场景节点级裁剪：node 自身标了 spoiler 且 node_id 未揭示 → 抹 read_aloud / gm_notes 里的
/// secret_terms。没标或已揭示 → clone 原样（别太严）。
fn guard_node_text(node: &ScenarioNode, revealed: &HashSet<String>) -> ScenarioNode {
    let mut out = node.clone();
    if node.spoiler.is_empty() || revealed.contains(&node.node_id) {
        return out;
    }
    if let Some(ra) = &node.read_aloud {
        out.read_aloud = Some(node.spoiler.redact(ra));
    }
    if let Some(g) = &node.gm_notes {
        out.gm_notes = Some(node.spoiler.redact(g));
    }
    out
}

/// 实体级裁剪：实体标了 spoiler 且其 id 未揭示 → 换显示名为 public_alias、抹 body/summary
/// 里的 secret_terms、附 GM 揭示提示。没标 spoiler 或已揭示 → **字节原样透传**。
fn guard_entity(v: &Value, revealed: &HashSet<String>) -> Value {
    let id = v.get("id").and_then(Value::as_str).unwrap_or("");
    let spoiler = SpoilerMeta::from_value(v);
    if spoiler.is_empty() || (!id.is_empty() && revealed.contains(id)) {
        return v.clone();
    }
    let mut out = v.clone();
    if let Some(name) = v.get("name").and_then(Value::as_str) {
        out["name"] = json!(spoiler.safe_name(name));
    }
    let hint = reveal_hint(&spoiler);
    let mut hint_attached = false;
    for field in ["body", "summary"] {
        if let Some(t) = v.get(field).and_then(Value::as_str) {
            let mut red = spoiler.redact(t);
            if !hint_attached {
                red.push_str(&hint);
                hint_attached = true;
            }
            out[field] = json!(red);
        }
    }
    out
}

/// GM-only 揭示提示（reveal_conditions 自身过裁剪，防条件文案夹带剧透词）。
fn reveal_hint(spoiler: &SpoilerMeta) -> String {
    let conds = spoiler.redact(&spoiler.reveal_conditions.join("；"));
    if conds.trim().is_empty() {
        " （⚠️GM：该实体含未揭示信息，揭示前请用公开称呼指代，勿泄露隐藏内容）".into()
    } else {
        format!(" （⚠️GM：该实体含未揭示信息，揭示条件：{conds}；揭示前请用公开称呼指代，勿泄露）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_projection::scene_node_to_blocks;
    use trpg_model::SceneExtractionStatus;

    fn butler_scene() -> (ScenarioNode, Vec<Value>) {
        let mut n = ScenarioNode::default();
        n.node_id = "sc01".into();
        n.title = "宅邸大厅".into();
        n.read_aloud = Some("管家詹姆斯为你开门。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.referenced_npc_ids = vec!["npc_butler".into()];
        let npcs = vec![json!({
            "id": "npc_butler",
            "name": "管家詹姆斯",
            "body": "他其实是连环杀手，真名莫里亚蒂教授。",
            "spoiler": {
                "secret_terms": ["连环杀手", "莫里亚蒂教授"],
                "public_aliases": ["管家詹姆斯"],
                "reveal_conditions": ["在地窖发现尸体后"]
            }
        })];
        (n, npcs)
    }

    /// acceptance #2：未揭示 → GM context 有公开别名、无 secret_term；
    /// 账本更新（fact 揭示）后，secret 出现。
    #[test]
    fn unrevealed_entity_secret_trimmed_then_appears_after_reveal() {
        let (n, npcs) = butler_scene();

        // 未揭示：裁剪后投影含别名、不含任何 secret_term。
        let none: HashSet<String> = HashSet::new();
        let (gn, gnpcs) = guard_scene(&n, &npcs, &none);
        let text = scene_node_to_blocks("mod1", &gn, &gnpcs, &[])[0]
            .content
            .render_text();
        assert!(text.contains("管家詹姆斯"), "公开别名应在: {text}");
        assert!(!text.contains("连环杀手"), "secret_term 必被裁: {text}");
        assert!(!text.contains("莫里亚蒂教授"), "secret_term 必被裁: {text}");

        // 揭示后（账本含 npc_butler）：secret 原文出现。
        let revealed = HashSet::from(["npc_butler".to_string()]);
        let (gn2, gnpcs2) = guard_scene(&n, &npcs, &revealed);
        let text2 = scene_node_to_blocks("mod1", &gn2, &gnpcs2, &[])[0]
            .content
            .render_text();
        assert!(text2.contains("连环杀手"), "揭示后 secret 应出现: {text2}");
        assert!(
            text2.contains("莫里亚蒂教授"),
            "揭示后 secret 应出现: {text2}"
        );
    }

    /// 别太严：实体无 spoiler → guard 字节原样透传（绝不裁可玩内容）。
    #[test]
    fn entity_without_spoiler_passes_through_untouched() {
        let plain = json!({"id": "npc_clerk", "name": "店员", "body": "他热情地招呼你。"});
        let out = guard_entity(&plain, &HashSet::new());
        assert_eq!(out, plain, "无 spoiler 实体必字节原样");
    }

    /// 场景节点级剧透：node_id 未揭示 → read_aloud/gm_notes 里的 secret_terms 被裁；
    /// node_id 揭示后 → 原文出现。
    #[test]
    fn scene_level_secret_trimmed_until_node_revealed() {
        let mut n = ScenarioNode::default();
        n.node_id = "sc_cellar".into();
        n.title = "地窖".into();
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.read_aloud = Some("一切看似平常。".into());
        n.gm_notes = Some("墙后藏着传送门，通往异界。".into());
        n.spoiler = SpoilerMeta {
            secret_terms: vec!["传送门".into(), "异界".into()],
            public_aliases: vec![],
            reveal_conditions: vec!["推开石墙后".into()],
        };

        let none: HashSet<String> = HashSet::new();
        let (gn, _) = guard_scene(&n, &[], &none);
        let text = scene_node_to_blocks("mod1", &gn, &[], &[])[0]
            .content
            .render_text();
        assert!(!text.contains("传送门"), "场景级 secret 必被裁: {text}");
        assert!(!text.contains("异界"), "场景级 secret 必被裁: {text}");
        assert!(text.contains("墙后藏着"), "非剧透 gm_notes 保留: {text}");

        let revealed = HashSet::from(["sc_cellar".to_string()]);
        let (gn2, _) = guard_scene(&n, &[], &revealed);
        let text2 = scene_node_to_blocks("mod1", &gn2, &[], &[])[0]
            .content
            .render_text();
        assert!(
            text2.contains("传送门"),
            "节点揭示后 secret 应出现: {text2}"
        );
    }

    /// Knowledge P0a 回归：一个场景里**同时**含节点级剧透与实体级剧透时，
    /// revealed 账本里同时带 node_id 与 entity_id → 两者都不再被裁（揭示放行）；
    /// 账本为空 → 两者都被裁（未揭示裁剪）。证明 reveal_fact 落账的 node/entity id
    /// 经 guard_scene 后确实不被 redact（enforcement 半的放行语义）。
    ///
    /// 断言走渲染路（scene_node_to_blocks().render_text()）而非裸 JSON to_string()：
    /// guard_entity 只裁 body/summary/name，**有意保留** spoiler 元数据块（secret_terms
    /// 数组本身仍在 JSON 里），裸 to_string 会恒含 secret_term → 假失败。渲染路是
    /// 实际进 GM context 的文本，与既有两条 spoiler_guard 测试同口径。
    #[test]
    fn revealed_node_and_entity_are_not_redacted() {
        // 节点级 secret 选 "传送门"：避开 butler npc 的 reveal_conditions("在地窖发现
        // 尸体后") 词汇——否则该条件文案只按 npc secret_terms 裁，节点词会经 GM hint 漏出。
        let (mut n, npcs) = butler_scene();
        n.spoiler.secret_terms = vec!["传送门".to_string()];
        n.gm_notes = Some("阁楼藏着传送门。".to_string());

        // 未揭示：渲染后节点级 secret("传送门") 与实体级 secret("莫里亚蒂教授") 都被裁。
        let none = HashSet::new();
        let (hn, hnpcs) = guard_scene(&n, &npcs, &none);
        let hidden = scene_node_to_blocks("mod1", &hn, &hnpcs, &[])[0]
            .content
            .render_text();
        assert!(
            !hidden.contains("传送门"),
            "未揭示 → 节点级 secret 必被裁: {hidden}"
        );
        assert!(
            !hidden.contains("莫里亚蒂教授"),
            "未揭示 → 实体级 secret 必被裁: {hidden}"
        );

        // revealed 账本同时带 node_id(sc01) 与 entity_id(npc_butler) → 两者都放行。
        let revealed = HashSet::from(["sc01".to_string(), "npc_butler".to_string()]);
        let (on, onpcs) = guard_scene(&n, &npcs, &revealed);
        let open = scene_node_to_blocks("mod1", &on, &onpcs, &[])[0]
            .content
            .render_text();
        assert!(
            open.contains("传送门"),
            "node_id 揭示后 → 节点级 secret 放行: {open}"
        );
        assert!(
            open.contains("莫里亚蒂教授"),
            "entity_id 揭示后 → 实体级 secret 放行: {open}"
        );
    }

    /// Knowledge P0a 回归：一个场景里**同时**含节点级剧透与实体级剧透时，
    /// revealed 账本里同时带 node_id 与 entity_id → 两者都不再被裁（揭示放行）；
    /// 账本为空 → 两者都被裁（未揭示裁剪）。证明 reveal_fact 落账的 node/entity id
    /// 经 guard_scene 后确实不被 redact（enforcement 半的放行语义）。
    ///
    /// 断言走渲染路（scene_node_to_blocks().render_text()）而非裸 JSON to_string()：
    /// guard_entity 只裁 body/summary/name，**有意保留** spoiler 元数据块（secret_terms
    /// 数组本身仍在 JSON 里），裸 to_string 会恒含 secret_term → 假失败。渲染路是
    /// 实际进 GM context 的文本，与既有两条 spoiler_guard 测试同口径。
    #[test]
    fn revealed_node_and_entity_are_not_redacted() {
        // 节点级 secret 选 "传送门"：避开 butler npc 的 reveal_conditions("在地窖发现
        // 尸体后") 词汇——否则该条件文案只按 npc secret_terms 裁，节点词会经 GM hint 漏出。
        let (mut n, npcs) = butler_scene();
        n.spoiler.secret_terms = vec!["传送门".to_string()];
        n.gm_notes = Some("阁楼藏着传送门。".to_string());

        // 未揭示：渲染后节点级 secret("传送门") 与实体级 secret("莫里亚蒂教授") 都被裁。
        let none = HashSet::new();
        let (hn, hnpcs) = guard_scene(&n, &npcs, &none);
        let hidden = scene_node_to_blocks("mod1", &hn, &hnpcs, &[])[0]
            .content
            .render_text();
        assert!(
            !hidden.contains("传送门"),
            "未揭示 → 节点级 secret 必被裁: {hidden}"
        );
        assert!(
            !hidden.contains("莫里亚蒂教授"),
            "未揭示 → 实体级 secret 必被裁: {hidden}"
        );

        // revealed 账本同时带 node_id(sc01) 与 entity_id(npc_butler) → 两者都放行。
        let revealed = HashSet::from(["sc01".to_string(), "npc_butler".to_string()]);
        let (on, onpcs) = guard_scene(&n, &npcs, &revealed);
        let open = scene_node_to_blocks("mod1", &on, &onpcs, &[])[0]
            .content
            .render_text();
        assert!(
            open.contains("传送门"),
            "node_id 揭示后 → 节点级 secret 放行: {open}"
        );
        assert!(
            open.contains("莫里亚蒂教授"),
            "entity_id 揭示后 → 实体级 secret 放行: {open}"
        );
    }

    /// 别太严回归：整张 npcs 都无 spoiler 时，guard_scene 产出与原 npcs 字节等价，
    /// node 也未被改（保证零误裁、与既有投影路径行为一致）。
    #[test]
    fn no_spoiler_anywhere_is_byte_identical() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.title = "加油站".into();
        n.read_aloud = Some("你们看到褪色的广告牌。".into());
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        let npcs = vec![json!({"id": "npc1", "name": "拉斯", "summary": "老板"})];
        let (gn, gnpcs) = guard_scene(&n, &npcs, &HashSet::new());
        assert_eq!(
            serde_json::to_value(&gn).unwrap(),
            serde_json::to_value(&n).unwrap(),
            "node 字节不变"
        );
        assert_eq!(gnpcs, npcs, "npcs 字节不变");
    }
}
