//! MAT.M9b / DP-A — 授权 NPC 证词 & 场景知识 SURFACE（Enforce 门控、source-anchored）。
//!
//! 架构师 Q3 DP-A 拍板:把 DP-1=4b 可采纳的合成**源**从(始终为空的)结构化
//! `facts_can_reveal` 字段,放宽到模组**确实存在的原文** —— 当前场景的
//! `read_aloud`/`gm_notes` + 每个 active NPC 的 `body` —— 在**同样的三不变量**
//! (① 不与 `gm_truth` 抵触:此处即原文本身,天然不抵触;② proposal-only / GmOnly:
//! 玩家不可见的 GM steering,披露仍走既有 reveal + presentation 闸;③ 剧透裁剪 +
//! 既有 known-to-player 闸)下采纳。**Source-anchored**:逐字模组原文,不杜撰原文之外的事实
//! —— "NPC 能揭示的内容" 就是模组关于它的原文 + 场景原文。
//!
//! 镜像 M9a 线索 surface + M8 把 NPC body 折进 persona:把上述原文(剧透裁剪后)渲染为一个
//! **GmOnly** 上下文块,令在场 NPC 在玩家**engage + 赢得检定**时,有原文素材作具体证词可
//! 揭示(而非停在"愿意多说一点");切勿主动奉送、切勿杜撰。
//!
//! # 加性 / 字节级基线
//! 仅 `Enforce` 产块(调用方 `prepare_turn_context` 侧 `is_enforce()` 闸);`Off`/`Shadow`
//! 不调用本路径 → OFF 字节等价基线。纯函数无副作用、不落库。
//!
//! # 零规则集硬编码
//! 全 data-driven(typed 场景字段 + NPC body 经 [`entity_prose`]),绝不按 `ruleset_id`/
//! `module_id` 分支。

use trpg_model::{entity_prose, ModuleBundle, ScenarioNode, SpoilerMeta};

/// 一条已抽出的授权知识源(纯数据,正文为剧透裁剪后的模组原文)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestimonySource {
    /// 来源标签(NPC 名 或 "场景知识")。
    pub label: String,
    /// 正文(source-backed、剧透裁剪后)。
    pub text: String,
}

/// 把场景的 source-present GM/念白原文剧透裁剪后收集成一条知识源。
/// 空 / 裁剪后为空 ⇒ None(fail-closed,绝不产空块)。
fn scene_knowledge(scene: &ScenarioNode) -> Option<TestimonySource> {
    let mut parts: Vec<String> = Vec::new();
    for raw in [scene.read_aloud.as_deref(), scene.gm_notes.as_deref()] {
        let Some(t) = raw else { continue };
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        // 场景级剧透词(secret_terms)在进入任何下游前裁剪;非剧透场景 → 逐字透传。
        let red = scene.spoiler.redact(t);
        let red = red.trim();
        if !red.is_empty() {
            parts.push(red.to_string());
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(TestimonySource {
        label: "场景知识 / scene knowledge".to_string(),
        text: parts.join("\n"),
    })
}

/// 把一个 active NPC 的 source-present body 原文剧透裁剪后收集成一条知识源。
/// 无 body / 无名 / 裁剪后为空 ⇒ None(fail-closed)。该实体自己声明的 secret_terms
/// 在折入前裁剪(同 M8 防御纵深)。
fn npc_knowledge(entry: &serde_json::Value) -> Option<TestimonySource> {
    let name = entry
        .get("name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let raw = entity_prose::entity_body_prose(entry)?.trim();
    if raw.is_empty() {
        return None;
    }
    let red = SpoilerMeta::from_value(entry).redact(raw);
    let red = red.trim();
    if red.is_empty() {
        return None;
    }
    Some(TestimonySource {
        label: name.to_string(),
        text: red.to_string(),
    })
}

/// 定位当前场景节点(以 `module_id` 过滤模组,再在 `module_graph.scenes` 中按 `scene_id` 查)。
fn find_current_scene<'a>(
    modules: &'a [ModuleBundle],
    module_id: &str,
    scene_id: &str,
) -> Option<&'a ScenarioNode> {
    modules
        .iter()
        .find(|m| m.module_id == module_id)
        .and_then(|m| m.module_graph.scenes.iter().find(|s| s.node_id == scene_id))
}

/// 收集本回合可作证词素材的授权知识源:当前场景知识 + 每个 active NPC 的 body 原文。
/// generic、fail-closed(任一缺失项跳过,不报错)。无模组 ⇒ 空。
pub fn collect_testimony_sources(
    modules: &[ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
    active_npc_ids: &[String],
) -> Vec<TestimonySource> {
    let Some(mid) = module_id else {
        return Vec::new();
    };
    let mut out: Vec<TestimonySource> = Vec::new();
    // 场景知识(架构师 DP-A 新增源)。
    if let Some(sid) = scene_id {
        if let Some(scene) = find_current_scene(modules, mid, sid) {
            if let Some(src) = scene_knowledge(scene) {
                out.push(src);
            }
        }
    }
    // 每个 active NPC 的 body 原文(DP-A 的 per-NPC 半)。
    if let Some(module) = modules.iter().find(|m| m.module_id == mid) {
        for npc_id in active_npc_ids {
            let Some(entry) = module.module_graph.npcs.iter().find(|v| {
                v.get("id").and_then(|x| x.as_str()) == Some(npc_id.as_str())
                    || v.get("actor_id").and_then(|x| x.as_str()) == Some(npc_id.as_str())
            }) else {
                continue;
            };
            if let Some(src) = npc_knowledge(entry) {
                // 同名去重(场景知识标签固定,NPC 标签为名)。
                if !out
                    .iter()
                    .any(|e| e.label == src.label && e.text == src.text)
                {
                    out.push(src);
                }
            }
        }
    }
    out
}

/// 把授权知识源渲染成一段 GM 面文本(框架文案:仅当玩家 engage + 赢得检定才据原文揭示
/// 具体证词,不主动奉送、不杜撰、不一次性倾倒)。
pub fn render_testimony_surface(sources: &[TestimonySource]) -> String {
    let mut s = String::from(
        "【可揭示证词 / Authored NPC & scene knowledge — GM 内部素材】\n\
         以下均为模组原文(source-backed)。当在场 NPC 被玩家 engage 且玩家通过一次**成功的**\
         相关检定(交涉 / 心理学 / Persuasion / 侦查 等)时,据其内容让该 NPC 说出**具体的、\
         原文支撑的**证词或信息 —— 不要只停在『他似乎愿意多说一点』,要给出实质内容;但**切勿**\
         主动奉送未赢得的信息、**切勿**一次性倾倒、**切勿**杜撰原文之外的事实。剧透词已在源头\
         裁剪。\n",
    );
    for (i, src) in sources.iter().enumerate() {
        s.push_str(&format!("{}. [{}] {}\n", i + 1, src.label, src.text));
    }
    s
}

/// 便利:为指定模组+场景+active NPC 集渲染证词 surface 文本。无任何源 ⇒ None。
/// (Enforce 门控在调用方 `prepare_turn_context`,与 M2/M8/M9a 一致。)
pub fn module_testimony_surface_text(
    modules: &[ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
    active_npc_ids: &[String],
) -> Option<String> {
    let sources = collect_testimony_sources(modules, module_id, scene_id, active_npc_ids);
    if sources.is_empty() {
        return None;
    }
    Some(render_testimony_surface(&sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{ModuleBundle, ModuleGraph, ScenarioNode};

    fn scene(node_id: &str, read_aloud: Option<&str>, gm_notes: Option<&str>) -> ScenarioNode {
        ScenarioNode {
            node_id: node_id.to_string(),
            title: "T".into(),
            node_type: "scene".into(),
            summary: "S".into(),
            read_aloud: read_aloud.map(str::to_string),
            gm_notes: gm_notes.map(str::to_string),
            ..Default::default()
        }
    }

    fn module(scenes: Vec<ScenarioNode>, npcs: Vec<serde_json::Value>) -> ModuleBundle {
        let graph = ModuleGraph {
            module_id: "m1".into(),
            scenes,
            npcs,
            ..Default::default()
        };
        ModuleBundle {
            schema_version: String::new(),
            bundle_id: String::new(),
            module_id: "m1".into(),
            ruleset_id: None,
            title: String::new(),
            source_index: Default::default(),
            module_graph: graph,
            module_prep_packets: vec![],
            module_locators: vec![],
            material_index: vec![],
            context_blocks: vec![],
            validation_report: Default::default(),
            conversion_trace: vec![],
        }
    }

    #[test]
    fn scene_gm_notes_and_read_aloud_surfaced() {
        // DP-A: 当前场景的 source-present read_aloud + gm_notes 进证词块。
        let m = module(
            vec![scene(
                "sc1",
                Some("门半开着。"),
                Some("门后是凶手的藏身处。"),
            )],
            vec![],
        );
        let srcs = collect_testimony_sources(&[m], Some("m1"), Some("sc1"), &[]);
        assert_eq!(srcs.len(), 1);
        assert!(srcs[0].text.contains("门半开着。"));
        assert!(srcs[0].text.contains("门后是凶手的藏身处。"));
    }

    #[test]
    fn active_npc_body_surfaced() {
        // DP-A per-NPC 半:active NPC 的 body 原文进证词块。
        let m = module(
            vec![],
            vec![json!({"id": "npc_russ", "name": "拉斯", "body": "他是加油站老板,见过那辆车。"})],
        );
        let srcs = collect_testimony_sources(&[m], Some("m1"), None, &["npc_russ".to_string()]);
        assert_eq!(srcs.len(), 1);
        assert_eq!(srcs[0].label, "拉斯");
        assert!(srcs[0].text.contains("见过那辆车"));
    }

    #[test]
    fn scene_secret_terms_redacted() {
        // 不变量③:场景 secret_terms 在进块前裁剪。
        let mut sc = scene("sc1", None, Some("真凶是管家莫里亚蒂教授。"));
        sc.spoiler = SpoilerMeta::from_value(&json!({
            "spoiler": {"secret_terms": ["莫里亚蒂教授"]}
        }));
        let m = module(vec![sc], vec![]);
        let srcs = collect_testimony_sources(&[m], Some("m1"), Some("sc1"), &[]);
        assert_eq!(srcs.len(), 1);
        assert!(
            !srcs[0].text.contains("莫里亚蒂教授"),
            "secret redacted: {}",
            srcs[0].text
        );
    }

    #[test]
    fn npc_secret_terms_redacted() {
        let m = module(
            vec![],
            vec![json!({
                "id": "npc_butler", "name": "管家",
                "body": "他其实是连环杀手。",
                "spoiler": {"secret_terms": ["连环杀手"]}
            })],
        );
        let srcs = collect_testimony_sources(&[m], Some("m1"), None, &["npc_butler".to_string()]);
        assert_eq!(srcs.len(), 1);
        assert!(
            !srcs[0].text.contains("连环杀手"),
            "secret redacted: {}",
            srcs[0].text
        );
    }

    #[test]
    fn empty_scene_and_no_npc_yields_nothing() {
        let m = module(vec![scene("sc1", None, None)], vec![]);
        let srcs = collect_testimony_sources(&[m], Some("m1"), Some("sc1"), &[]);
        assert!(srcs.is_empty());
        assert!(
            module_testimony_surface_text(&[module(vec![], vec![])], Some("m1"), None, &[])
                .is_none()
        );
    }

    #[test]
    fn page_stub_npc_no_body_does_not_testify() {
        // codex narrowing: page-stub NPC(无 body)fail-closed,不证词。
        let m = module(
            vec![],
            vec![json!({"id": "npc_stub", "name": "路人", "page": 12})],
        );
        let srcs = collect_testimony_sources(&[m], Some("m1"), None, &["npc_stub".to_string()]);
        assert!(srcs.is_empty());
    }

    #[test]
    fn unknown_module_yields_nothing() {
        let m = module(vec![scene("sc1", Some("x"), None)], vec![]);
        assert!(collect_testimony_sources(&[m], Some("other"), Some("sc1"), &[]).is_empty());
        assert!(module_testimony_surface_text(&[], Some("m1"), None, &[]).is_none());
    }

    #[test]
    fn render_includes_earn_framing_and_content() {
        let srcs = vec![TestimonySource {
            label: "拉斯".into(),
            text: "见过那辆车。".into(),
        }];
        let r = render_testimony_surface(&srcs);
        assert!(r.contains("赢得") || r.contains("成功"));
        assert!(r.contains("切勿") && r.contains("杜撰"));
        assert!(r.contains("见过那辆车。"));
        assert!(r.contains("[拉斯]"));
    }

    #[test]
    fn generic_no_ruleset_or_module_branch() {
        // 同一函数对任意 module_id 行为一致(无名分支)。
        let m1 = module(vec![scene("s", Some("a1"), None)], vec![]);
        let m2 = module(vec![scene("s", Some("a1"), None)], vec![]);
        let r1 = collect_testimony_sources(&[m1], Some("m1"), Some("s"), &[]);
        let r2 = collect_testimony_sources(&[m2], Some("m1"), Some("s"), &[]);
        assert_eq!(r1, r2);
    }
}
