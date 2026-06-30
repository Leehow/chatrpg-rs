//! MAT.M9a — 授权线索 SURFACE（Enforce 门控、source-anchored）。
//!
//! 图谱线索是无正文骨架(`{id,name,page,content_class}`)、场景 `referenced_clue_ids` 多为空，
//! 真正可发现的线索正文在**模组 prep packet** 的 `current_session_packet` 里
//! (CoC `clues[]{id,text,tier,delivery}`；Cyberpunk `clue_web[]{clue,points_to}`)。
//!
//! 本模块从 prep packet 抽取**玩家可见**线索(剔除任何 `gm_only*` 层级)，渲染成一段
//! source-backed 文本，由 `prepare_turn_context` 在 `Enforce` 下包成一个 **GmOnly** 上下文块
//! 注入 GM——令 GM 在玩家通过一次**成功的调查类检定**赢得线索时，有原文素材可揭示
//! (镜像 M8 把 NPC body 折进 persona 的做法；不主动奉送靠框架文案 + 既有 presentation 闸 +
//! M4 known-to-player 闸兜底；secret 在源头按层级剔除 = fail-closed 防剧透)。
//!
//! # 加性 / 字节级基线
//! 仅 `Enforce` 产块——`Off`/`Shadow` 不调用本路径(prepare_turn_context 侧 `is_enforce()` 闸)，
//! 故 OFF 字节等价基线。纯函数无副作用、不落库。
//!
//! # 零规则集硬编码
//! schema 读取全 data-driven(多备选字段名),绝不按 `ruleset_id`/`module_id` 分支。

use serde_json::Value;
use trpg_model::{ModuleBundle, ModuleGraph};

/// 一条已抽出的玩家可见线索(纯数据,正文为模组原文)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfacedClue {
    /// 线索 id(原文 `id` 或按出现序合成 `clue_<idx>`)。
    pub id: String,
    /// 线索正文(source-backed,模组原文)。
    pub text: String,
    /// 投递技能提示(如 "侦查、常识"),用于 GM 判断哪类检定可揭示;缺失则 None。
    pub delivery_hint: Option<String>,
}

/// 取一组备选键里第一个非空字符串值(trim 后)。
fn first_nonempty(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// 判断一条线索是否 GM-only(剧透/种子),据层级/可见性字段的 gm_only/secret 标记 + 布尔旗。
/// fail-closed(codex 折入):负向字符串匹配易漏未标记 secret,故同时:
/// ① 受检多字段含 gm_only/keeper_only/secret/spoiler/hidden ⇒ 剔除;
/// ② 布尔旗 `secret`/`hidden`/`spoiler`/`gm_only`==true ⇒ 剔除。
fn is_gm_only(clue: &Value) -> bool {
    for k in [
        "tier",
        "visibility",
        "audience",
        "access",
        "delivery",
        "scope",
    ] {
        if let Some(s) = clue.get(k).and_then(Value::as_str) {
            let s = s.to_lowercase();
            if s.contains("gm_only")
                || s.contains("gm-only")
                || s.contains("gmonly")
                || s.contains("gm only")
                || s.contains("keeper_only")
                || s.contains("keeper only")
                || s.contains("secret")
                || s.contains("spoiler")
                || s == "gm"
                || s == "hidden"
            {
                return true;
            }
        }
    }
    for k in ["secret", "hidden", "spoiler", "gm_only"] {
        if clue.get(k).and_then(Value::as_bool) == Some(true) {
            return true;
        }
    }
    false
}

/// 从单条线索 Value 抽出 [`SurfacedClue`](玩家可见才返回);GM-only / 无正文 ⇒ None。
fn extract_clue(clue: &Value, fallback_id: &str) -> Option<SurfacedClue> {
    if is_gm_only(clue) {
        return None;
    }
    // codex 折入:仅取**线索专属**正文字段(text/clue/discovery/meaning),不取过宽的
    // content/description(可能是 GM 笔记/摘要而非玩家面线索原文)→ 守 rule9 no-invention 边界。
    let text = first_nonempty(clue, &["text", "clue", "discovery", "meaning"])?;
    let id = clue
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_id.to_string());
    let delivery_hint = first_nonempty(clue, &["delivery", "skill", "via", "method"]);
    Some(SurfacedClue {
        id,
        text,
        delivery_hint,
    })
}

fn graph_clue_id(clue: &Value) -> Option<String> {
    clue.get("id")
        .or_else(|| clue.get("clue_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Graph-level clues often carry authored summaries rather than prep-packet `text`.
/// They are still source-backed, but only the current scene's referenced clues are
/// eligible for this surface block.
fn extract_graph_clue(clue: &Value, fallback_id: &str) -> Option<SurfacedClue> {
    if is_gm_only(clue) {
        return None;
    }
    let text = first_nonempty(clue, &["text", "clue", "discovery", "meaning", "summary"])?;
    let id = graph_clue_id(clue).unwrap_or_else(|| fallback_id.to_string());
    let delivery_hint = first_nonempty(clue, &["delivery", "skill", "via", "method"]);
    Some(SurfacedClue {
        id,
        text,
        delivery_hint,
    })
}

/// 从 `current_session_packet`(Value)抽取玩家可见线索。generic schema:
/// `clues[]` / `clue_web[]` / `current_scenes[].clues[]`,逐条 [`extract_clue`]。
/// 按出现序合成 fallback id;GM-only 与无正文条目剔除。
pub fn surface_player_facing_clues(current_session_packet: &Value) -> Vec<SurfacedClue> {
    let mut out: Vec<SurfacedClue> = Vec::new();
    let mut idx = 0usize;
    let mut take = |arr_key: &str, scenes: bool| {
        if scenes {
            let Some(scene_arr) = current_session_packet
                .get("current_scenes")
                .and_then(Value::as_array)
            else {
                return;
            };
            for sc in scene_arr {
                if let Some(arr) = sc.get("clues").and_then(Value::as_array) {
                    for c in arr {
                        if let Some(sc) = extract_clue(c, &format!("scene_clue_{idx}")) {
                            if !out.iter().any(|e| e.id == sc.id) {
                                out.push(sc);
                            }
                        }
                        idx += 1;
                    }
                }
            }
            return;
        }
        if let Some(arr) = current_session_packet
            .get(arr_key)
            .and_then(Value::as_array)
        {
            for c in arr {
                if let Some(sc) = extract_clue(c, &format!("{arr_key}_{idx}")) {
                    if !out.iter().any(|e| e.id == sc.id) {
                        out.push(sc);
                    }
                }
                idx += 1;
            }
        }
    };
    take("clues", false);
    take("clue_web", false);
    take("", true);
    out
}

pub fn surface_current_scene_graph_clues(
    module: &ModuleBundle,
    scene_id: Option<&str>,
) -> Vec<SurfacedClue> {
    let Some(scene_id) = scene_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let mut graph: ModuleGraph = module.module_graph.clone();
    let _ = crate::clue_projection::project_clues_onto_scenes(&mut graph);
    let Some(scene) = graph
        .scenes
        .iter()
        .find(|scene| scene.node_id.trim() == scene_id)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for clue_ref in &scene.referenced_clue_ids {
        let cid = clue_ref.trim();
        if cid.is_empty() {
            continue;
        }
        let Some(clue) = graph
            .clues
            .iter()
            .find(|clue| graph_clue_id(clue).as_deref() == Some(cid))
        else {
            continue;
        };
        if let Some(sc) = extract_graph_clue(clue, cid) {
            if !out
                .iter()
                .any(|existing: &SurfacedClue| existing.id == sc.id)
            {
                out.push(sc);
            }
        }
    }
    out
}

/// 把玩家可见线索渲染成一段 GM 面文本(框架文案明令:仅当玩家赢得检定才揭示、不主动奉送)。
pub fn render_clue_surface(clues: &[SurfacedClue]) -> String {
    let mut s = String::from(
        "【可揭示线索 / Authored discoverable clues — GM 内部素材】\n\
         以下线索均来自模组原文(source-backed)。**仅当**玩家通过一次**成功的相关调查类检定**\
         (侦查 / Spot Hidden / Perception / 搜查 / 观察 / 调查 等)赢得线索时,才据其内容向玩家\
         揭示;切勿主动奉送、切勿一次性倾倒、切勿杜撰原文之外的内容。GM-only / 种子线索已在源头\
         剔除,不在此列。\n",
    );
    for (i, c) in clues.iter().enumerate() {
        s.push_str(&format!("{}. [{}]", i + 1, c.id));
        if let Some(d) = &c.delivery_hint {
            s.push_str(&format!("(投递:{d})"));
        }
        s.push_str(&format!(" {}\n", c.text));
    }
    s
}

/// 便利:为指定模组从其 prep packet 集渲染线索 surface 文本。无模组 / 无玩家可见线索 ⇒ None。
/// (Enforce 门控在调用方 `prepare_turn_context`,与 M2/M8 一致。)
pub fn module_clue_surface_text(
    modules: &[ModuleBundle],
    module_id: Option<&str>,
) -> Option<String> {
    let mid = module_id?;
    let module = modules.iter().find(|m| m.module_id == mid)?;
    let mut clues: Vec<SurfacedClue> = Vec::new();
    for packet in &module.module_prep_packets {
        for c in surface_player_facing_clues(&packet.current_session_packet) {
            if !clues.iter().any(|e| e.id == c.id) {
                clues.push(c);
            }
        }
    }
    if clues.is_empty() {
        return None;
    }
    Some(render_clue_surface(&clues))
}

pub fn module_scene_clue_surface_text(
    modules: &[ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
    prep_packet: Option<&Value>,
) -> Option<String> {
    let mid = module_id?;
    let module = modules.iter().find(|m| m.module_id == mid)?;
    let mut clues: Vec<SurfacedClue> = Vec::new();
    if let Some(packet) = prep_packet {
        for c in surface_player_facing_clues(packet) {
            if !clues.iter().any(|e| e.id == c.id) {
                clues.push(c);
            }
        }
    }
    for c in surface_current_scene_graph_clues(module, scene_id) {
        if !clues.iter().any(|e| e.id == c.id) {
            clues.push(c);
        }
    }
    if clues.is_empty() {
        return None;
    }
    Some(render_clue_surface(&clues))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{ModuleGraph, ScenarioNode};

    // ===== CoC schema: clues[]{id,text,tier,delivery} =====
    #[test]
    fn coc_clues_player_facing_extracted_gm_only_withheld() {
        let packet = json!({
            "clues": [
                {"id":"clue_1","text":"阿巴托尔依附于异常本地循环。","tier":"obvious","delivery":"环境描写、玩家观察"},
                {"id":"clue_3","text":"小镇破败得不合理。","tier":"atmospheric","delivery":"侦查、常识"},
                {"id":"clue_5","text":"敌人会更快推进猎杀计划。","tier":"gm_only_seed","delivery":"GM反应逻辑"}
            ]
        });
        let out = surface_player_facing_clues(&packet);
        let ids: Vec<&str> = out.iter().map(|c| c.id.as_str()).collect();
        assert!(
            ids.contains(&"clue_1") && ids.contains(&"clue_3"),
            "玩家可见线索应抽出: {ids:?}"
        );
        assert!(!ids.contains(&"clue_5"), "gm_only_seed 必须剔除: {ids:?}");
        let c1 = out.iter().find(|c| c.id == "clue_1").unwrap();
        assert_eq!(c1.delivery_hint.as_deref(), Some("环境描写、玩家观察"));
        assert!(c1.text.contains("阿巴托尔"), "正文须为模组原文");
    }

    // ===== Cyber schema: clue_web[]{clue,points_to} (无 id/tier/delivery) =====
    #[test]
    fn cyber_clue_web_extracted_with_synth_ids() {
        let packet = json!({
            "clue_web": [
                {"clue":"Athena is attached to a cable into the warehouse","points_to":["external power"]},
                {"clue":"Athena's repeated plea for help","points_to":["victim not villain"]}
            ]
        });
        let out = surface_player_facing_clues(&packet);
        assert_eq!(out.len(), 2, "clue_web 两条都应抽出: {out:?}");
        assert!(
            out[0].text.contains("Athena is attached"),
            "clue 字段即正文"
        );
        assert!(
            out[0].id.starts_with("clue_web_"),
            "无 id 时合成: {}",
            out[0].id
        );
        assert!(out[0].delivery_hint.is_none(), "clue_web 无 delivery");
    }

    // ===== current_scenes[].clues[] 备选位置 =====
    #[test]
    fn current_scenes_clues_extracted() {
        let packet = json!({
            "current_scenes": [
                {"scene_id":"s1","clues":[{"id":"sc_a","discovery":"门后有血迹"}]},
                {"scene_id":"s2","clues":[{"meaning":"信纸缺了一角","tier":"social"}]}
            ]
        });
        let out = surface_player_facing_clues(&packet);
        assert_eq!(out.len(), 2, "场景内线索应抽出: {out:?}");
        assert_eq!(out[0].id, "sc_a");
        assert!(out[0].text.contains("血迹"));
        assert!(out[1].text.contains("信纸"), "meaning 作为正文备选");
    }

    // ===== fail-closed：无线索字段 / 空 packet → 空 =====
    #[test]
    fn empty_or_missing_yields_empty() {
        assert!(surface_player_facing_clues(&json!({})).is_empty());
        assert!(surface_player_facing_clues(&json!({"clues":[]})).is_empty());
        assert!(
            surface_player_facing_clues(&json!({"clues":[{"id":"x","tier":"obvious"}]})).is_empty(),
            "无正文字段 → 不抽"
        );
    }

    // ===== gm_only 多种写法都剔除（generic）=====
    #[test]
    fn gm_only_variants_all_withheld() {
        for marker in ["gm_only", "GM-only", "gmonly", "GM only", "keeper_only"] {
            let packet = json!({"clues":[{"id":"x","text":"秘密","tier":marker}]});
            assert!(
                surface_player_facing_clues(&packet).is_empty(),
                "tier={marker:?} 应剔除"
            );
        }
        // delivery 字段标 GM-only 也剔除
        let packet = json!({"clues":[{"id":"y","text":"秘密","delivery":"GM-only reaction"}]});
        assert!(
            surface_player_facing_clues(&packet).is_empty(),
            "delivery GM-only 应剔除"
        );
    }

    // ===== codex 折入：secret/hidden/spoiler 布尔旗 + secret 层级也剔除 =====
    #[test]
    fn secret_boolean_and_tier_markers_withheld() {
        for clue in [
            json!({"id":"a","text":"X","secret":true}),
            json!({"id":"b","text":"X","hidden":true}),
            json!({"id":"c","text":"X","spoiler":true}),
            json!({"id":"d","text":"X","tier":"secret"}),
            json!({"id":"e","text":"X","tier":"spoiler_only"}),
        ] {
            let packet = json!({ "clues": [clue.clone()] });
            assert!(
                surface_player_facing_clues(&packet).is_empty(),
                "secret 标记应剔除: {clue:?}"
            );
        }
    }

    // ===== codex 折入：过宽字段 content/description 不当作线索正文 =====
    #[test]
    fn broad_fields_not_treated_as_clue_text() {
        let packet = json!({"clues":[{"id":"x","content":"GM 笔记摘要","description":"区域概览"}]});
        assert!(
            surface_player_facing_clues(&packet).is_empty(),
            "content/description 非线索专属正文字段 → 不抽(守 rule9)"
        );
    }

    // ===== render：含框架文案 + 不杜撰 + 列出线索 =====
    #[test]
    fn render_includes_earn_framing_and_clue_text() {
        let clues = vec![SurfacedClue {
            id: "clue_1".into(),
            text: "门后有血迹".into(),
            delivery_hint: Some("侦查".into()),
        }];
        let s = render_clue_surface(&clues);
        assert!(
            s.contains("成功") && s.contains("赢得"),
            "须含'赢得检定才揭示'框架"
        );
        assert!(s.contains("切勿主动奉送"), "须含不主动奉送框架");
        assert!(s.contains("门后有血迹"), "须列出线索正文");
        assert!(s.contains("clue_1"));
    }

    // ===== module_clue_surface_text：按模组聚合，未知模组 → None =====
    #[test]
    fn module_text_none_for_unknown_module() {
        let modules: Vec<ModuleBundle> = vec![];
        assert!(module_clue_surface_text(&modules, Some("nope")).is_none());
        assert!(module_clue_surface_text(&modules, None).is_none());
    }

    fn module_with_scene_graph_clue() -> ModuleBundle {
        let mut scene = ScenarioNode::default();
        scene.node_id = "loc_hall_records".into();
        scene.title = "Hall of Records".into();
        scene.referenced_clue_ids = vec!["handout_7".into()];
        ModuleBundle {
            schema_version: String::new(),
            bundle_id: String::new(),
            module_id: "m1".into(),
            ruleset_id: None,
            title: String::new(),
            source_index: Default::default(),
            module_graph: ModuleGraph {
                module_id: "m1".into(),
                scenes: vec![scene],
                clues: vec![json!({
                    "clue_id": "handout_7",
                    "title": "Executor and Chapel Record",
                    "summary": "Corbitt’s executor was Reverend Michael Thomas of the Chapel of Contemplation; the chapel closed in 1912."
                })],
                ..Default::default()
            },
            module_prep_packets: vec![],
            module_locators: vec![],
            material_index: vec![],
            context_blocks: vec![],
            validation_report: Default::default(),
            conversion_trace: vec![],
        }
    }

    #[test]
    fn current_scene_graph_clue_summary_is_surfaceable() {
        let module = module_with_scene_graph_clue();
        let out = surface_current_scene_graph_clues(&module, Some("loc_hall_records"));

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "handout_7");
        assert!(out[0].text.contains("Reverend Michael Thomas"));
        assert!(out[0].text.contains("1912"));
    }

    #[test]
    fn module_scene_clue_surface_text_merges_prep_and_graph_clues() {
        let module = module_with_scene_graph_clue();
        let prep = json!({
            "clues": [
                {"id": "prep_clue", "text": "A prep-packet clue.", "tier": "obvious"}
            ]
        });
        let text = module_scene_clue_surface_text(
            &[module],
            Some("m1"),
            Some("loc_hall_records"),
            Some(&prep),
        )
        .expect("merged clue surface should render");

        assert!(text.contains("prep_clue"));
        assert!(text.contains("handout_7"));
        assert!(text.contains("Reverend Michael Thomas"));
        assert!(text.contains("1912"));
    }
}
