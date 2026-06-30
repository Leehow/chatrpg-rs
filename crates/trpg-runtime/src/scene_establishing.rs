//! Q-MODULE DP-A' / DP-B' — player-deliverable scene ESTABLISHING material.
//!
//! 架构师 Q4 DP-A' 拍板:当前场景的 **NON-secret** `read_aloud`(进场念白 / 定调 box-text)
//! 在**进场**时即可作为**无门 establishing 散文**浮现 —— 让开场就有模组质感(具名场景 / 氛围),
//! 而不是泛化叙事。它与 A2 的"奖励门控证词"是两回事:
//!   - 本模块只取 `read_aloud`(作者写给玩家听的进场文本) —— **绝不**取 GM-only `gm_notes`,
//!     **绝不**碰 secrets / clues / 隐藏事实(那些仍走 A2 reveal + presentation 闸,本模块不动)。
//!   - 仍做场景级剧透裁剪(`scene.spoiler.redact`)作纵深防御:剧透场景的 read_aloud 会被裁。
//!
//! 输出是 source-anchored 的**素材**:由分体 Narrator 改写成画面(第二人称、织进场景),
//! **绝不**逐字倾倒 / 列清单 / 当选项菜单(尊重 EXAM_QUALITY_BAR Q-4 no-dump;渲染侧文案约束)。
//!
//! # 加性 / 字节级基线
//! 仅 `Enforce` 下调用方 `prepare_turn_context` 填充返回的 `CompiledContext.scene_establishing`;
//! Off/Shadow 不调用本路径 ⇒ 字段为空 ⇒ 分体 Narrator 注入空 ⇒ OFF 字节等价基线。纯函数无副作用。
//!
//! # 零规则集硬编码
//! 全 data-driven(typed 场景 `read_aloud` 字段),绝不按 `ruleset_id`/`module_id` 分支。

use serde_json::Value;
use trpg_model::{ModuleBundle, ScenarioNode};

/// player-facing 进场字段白名单(语义字段名,data-driven,**非** ruleset/module 名分支)。
/// 任何不在白名单里的字段(尤其 `*_gm_only` / `gm_only_*` / `secret*` / `redacted*`)绝不读取。
///
/// **fail-closed 收窄(codex A1 复核折入)**:只取**玩家进场即可感知的可观测现象 / 开场设定**:
/// - `elevator_pitch` **移除**:它常含谜底/真相(如「源头是被囚禁的异常体、Domain 正在扩张」)
///   = 剧透,绝不作进场素材。
/// - `read_aloud_style_prompt` **移除**:它是给 GM 的「怎么念 / 风格」指令(`*_style_prompt`),
///   不是世界内可感知正文,避免把风格指令提升为内容事实 / 泄露结构。
/// 保留的 `strong_start.{scene,immediate_hook,first_action_prompt}` + `mission_briefing.agency_intel`
/// 都是开场可观测现象(地点 / 异常表现 / 首个事件),不含谜底。谜底 / 线索仍走 A2 奖励门。
const PREP_ESTABLISHING_FIELDS: &[&str] = &[
    "strong_start.scene",
    "strong_start.immediate_hook",
    "strong_start.first_action_prompt",
];

/// 键名含这些标记 ⇒ 视为 GM-only,纵深防御直接跳过(即便误入白名单父对象)。
fn key_is_gm_only(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "gm_only",
        "keeper_only",
        "secret",
        "spoiler",
        "redacted",
        "hidden",
    ]
    .iter()
    .any(|m| k.contains(m))
}

/// 取 `a.b` 点路径下的非空字符串(trim 后);任一层键名 GM-only ⇒ None。
fn dotted_nonempty_str(root: &Value, path: &str) -> Option<String> {
    let mut cur = root;
    for seg in path.split('.') {
        if key_is_gm_only(seg) {
            return None;
        }
        cur = cur.get(seg)?;
    }
    let s = cur.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// anthology / spine 类模组(`graph.scenes` 为空或当前为 synthetic `spine:` 入口)无对应场景
/// 节点 ⇒ `collect_scene_establishing` 返回空。此时从 prep packet 的 `current_session_packet`
/// 抽取**非秘密**进场 establishing 素材(白名单玩家可见字段 + `mission_briefing.agency_intel[]`),
/// 让 anthology 开场同样有具名模组质感(FOUNT / Aquifer / Mercantile Avenue ……)。
///
/// fail-closed:GM-only 字段(`anomaly_profile_gm_only` / `gm_only_secrets_to_protect` /
/// `redacted_report_summary_gm_only` 等)不在白名单 ⇒ 绝不读取(by construction)。
/// 由分体 Narrator 改写成画面(尊重 Q-4 no-dump),绝不逐字倾倒。
pub fn prep_packet_establishing(current_session_packet: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in PREP_ESTABLISHING_FIELDS {
        if let Some(s) = dotted_nonempty_str(current_session_packet, path) {
            out.push(s);
        }
    }
    // mission_briefing.agency_intel[] — 写给玩家听的简报情报(其 gm_only 兄弟字段不读)。
    if let Some(mb) = current_session_packet.get("mission_briefing") {
        if let Some(arr) = mb.get("agency_intel").and_then(Value::as_array) {
            for it in arr {
                if let Some(s) = it.as_str() {
                    let t = s.trim();
                    if !t.is_empty() {
                        out.push(t.to_string());
                    }
                }
            }
        }
    }
    out
}

/// 在指定模组的场景图中按 `scene_id` 定位当前场景节点。
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

/// 收集本回合玩家可交付的进场 establishing 素材:当前场景的 NON-secret `read_aloud`
/// (剧透裁剪后)。fail-closed:无模组 / 无场景 / 无 read_aloud / 裁剪后为空 ⇒ 空 vec。
///
/// 返回多条切片(当前仅一条 = 场景念白),保持 `Vec<String>` 以便未来加入更多 player-safe
/// 来源(如已激活 NPC 的公开外观),且与 `NarrationPacket::with_scene_establishing` 契约一致。
pub fn collect_scene_establishing(
    modules: &[ModuleBundle],
    module_id: Option<&str>,
    scene_id: Option<&str>,
) -> Vec<String> {
    let (Some(mid), Some(sid)) = (module_id, scene_id) else {
        return Vec::new();
    };
    let Some(scene) = find_current_scene(modules, mid, sid) else {
        return Vec::new();
    };
    let Some(raw) = scene.read_aloud.as_deref() else {
        return Vec::new();
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    // 场景级剧透词在进入任何下游前裁剪;非剧透场景 ⇒ 逐字透传(read_aloud 本就是写给玩家的)。
    let red = scene.spoiler.redact(raw);
    let red = red.trim();
    if red.is_empty() {
        return Vec::new();
    }
    vec![red.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::{ModuleBundle, ModuleGraph, ScenarioNode, SpoilerMeta};

    fn scene(node_id: &str, read_aloud: Option<&str>, gm_notes: Option<&str>) -> ScenarioNode {
        ScenarioNode {
            node_id: node_id.to_string(),
            title: "门厅".into(),
            node_type: "scene".into(),
            summary: "S".into(),
            read_aloud: read_aloud.map(str::to_string),
            gm_notes: gm_notes.map(str::to_string),
            ..Default::default()
        }
    }

    fn module(scenes: Vec<ScenarioNode>) -> ModuleBundle {
        ModuleBundle {
            schema_version: String::new(),
            bundle_id: String::new(),
            module_id: "m1".into(),
            ruleset_id: None,
            title: String::new(),
            source_index: Default::default(),
            module_graph: ModuleGraph {
                module_id: "m1".into(),
                scenes,
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
    fn read_aloud_surfaced_as_establishing() {
        let m = module(vec![scene(
            "sc1",
            Some("春雨敲打着加油站锈蚀的雨棚，霓虹在水洼里碎成一片猩红。"),
            Some("GM-only：店主其实是线人。"),
        )]);
        let out = collect_scene_establishing(&[m], Some("m1"), Some("sc1"));
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("加油站"));
        // gm_notes (GM-only) must NEVER appear in player-deliverable establishing.
        assert!(!out[0].contains("线人"), "{}", out[0]);
    }

    #[test]
    fn scene_secret_terms_redacted() {
        let mut sc = scene("sc1", Some("门后站着真凶莫里亚蒂教授。"), None);
        sc.spoiler = SpoilerMeta::from_value(&json!({
            "spoiler": {"secret_terms": ["莫里亚蒂教授"]}
        }));
        let m = module(vec![sc]);
        let out = collect_scene_establishing(&[m], Some("m1"), Some("sc1"));
        assert_eq!(out.len(), 1);
        assert!(
            !out[0].contains("莫里亚蒂教授"),
            "secret redacted: {}",
            out[0]
        );
    }

    #[test]
    fn no_module_no_scene_no_read_aloud_yields_empty() {
        let m = module(vec![scene("sc1", None, Some("仅 GM 备注"))]);
        assert!(collect_scene_establishing(&[m.clone()], Some("m1"), Some("sc1")).is_empty());
        assert!(collect_scene_establishing(&[m.clone()], None, Some("sc1")).is_empty());
        assert!(collect_scene_establishing(&[m.clone()], Some("m1"), None).is_empty());
        assert!(collect_scene_establishing(&[m], Some("other"), Some("sc1")).is_empty());
        assert!(collect_scene_establishing(&[], Some("m1"), Some("sc1")).is_empty());
    }

    #[test]
    fn prep_packet_establishing_surfaces_player_facing_only() {
        let csp = json!({
            // elevator_pitch 含谜底(源头是被囚禁的异常体)= 剧透,绝不浮现。
            "elevator_pitch": "源头是一株被囚禁的植物异常体,其精华被蒸馏进化妆品。",
            "strong_start": {
                "scene": "Agency 晨会被 Aquifer 饱和打断",
                "immediate_hook": "Mercantile Avenue 出现异常聚集行为:自发哭泣、与旧友重逢。",
                "first_action_prompt": "特工被派往 Mercantile Avenue,靠近高端医美 FOUNT。",
                // read_aloud_style_prompt 是给 GM 的风格指令,不作 establishing 正文。
                "read_aloud_style_prompt": "开场风格:催眠感的瓷面与水珠倒流。"
            },
            "mission_briefing": {
                "agency_intel": [
                    "异常行为集中在 Mercantile Avenue。",
                    "可能的首个调查地点是 FOUNT 附近。"
                ],
                "redacted_report_summary_gm_only": "绝密：1500 年代的异常事件……"
            },
            "anomaly_profile_gm_only": "真相:植物异常体被囚禁榨取。",
            "gm_only_secrets_to_protect": ["Serena 是源头"]
        });
        let out = prep_packet_establishing(&csp);
        let joined = out.join("\n");
        // 可观测现象 / 开场设定浮现(具名地点 FOUNT / Mercantile Avenue)。
        assert!(joined.contains("FOUNT"), "{joined}");
        assert!(joined.contains("Mercantile Avenue"), "{joined}");
        assert!(joined.contains("自发哭泣"), "{joined}");
        // 谜底 / GM-only / 风格指令绝不出现。
        assert!(
            !joined.contains("被囚禁"),
            "spoiler/gm_only leaked: {joined}"
        );
        assert!(!joined.contains("Serena 是源头"), "secret leaked: {joined}");
        assert!(!joined.contains("1500"), "redacted leaked: {joined}");
        assert!(
            !joined.contains("催眠感"),
            "style_prompt leaked as content: {joined}"
        );
    }

    #[test]
    fn prep_packet_establishing_empty_when_no_player_fields() {
        let csp = json!({
            "anomaly_profile_gm_only": "x",
            "gm_only_secrets_to_protect": ["y"]
        });
        assert!(prep_packet_establishing(&csp).is_empty());
        assert!(prep_packet_establishing(&json!({})).is_empty());
    }

    #[test]
    fn generic_no_ruleset_or_module_branch() {
        let m1 = module(vec![scene("s", Some("同一念白"), None)]);
        let m2 = module(vec![scene("s", Some("同一念白"), None)]);
        assert_eq!(
            collect_scene_establishing(&[m1], Some("m1"), Some("s")),
            collect_scene_establishing(&[m2], Some("m1"), Some("s"))
        );
    }
}
