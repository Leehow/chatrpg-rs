//! 深抽提交里 scene_mechanics 的解析/过滤纯函数 + submit schema（任务 C2/C7）。
//! 从 module_reader.rs 拆出守 ≤400 行：这里只放确定性纯函数与 schema 数据，无 LLM。
use serde_json::{json, Value};
use trpg_model::SceneMechanicIntent;

/// submit_deep 里 `scene.scene_mechanics` 的 schema（单一事实源，C7）：字段形态与下游
/// 消费方逐字对齐——difficulty 对齐 scene_policy::difficulty_to_target 认的
/// {kind:"dv"|"static"|"target_number", value:<int>}（纯字符串不绑定结算目标，fail-closed
/// 走规则核默认）；effect_policy 条目对齐 trpg-model `EffectPatchIntent` 的 serde 形状
/// （tag="kind"，字段名错 → untagged Other → 折 unexecutable，不会真执行）。
/// 枚举值是引擎结构词表（非规则集分支）；对齐由本文件测试自检。
pub(crate) fn scene_mechanics_schema() -> Value {
    let effect_items = json!({"type": "object",
        "description": "EXACT field shapes per kind (wrong field names make the entry unexecutable): create_fact={kind,target,fact}; modify_track={kind,owner_kind,owner_id?,track_id,op,amount}; set_object_state={kind,object_id,patch}; start_countdown={kind,label,amount,scale,payload}",
        "properties": {
            "kind": {"type": "string", "enum": ["set_object_state", "modify_track", "create_fact", "start_countdown"]},
            "target": {"type": "string", "description": "create_fact: what/whom the fact is about"},
            "fact": {"type": "object", "description": "create_fact: the fact payload as an OBJECT (never a `value` string field)"},
            "owner_kind": {"type": "string", "enum": ["actor", "scene"], "description": "modify_track: who holds the track"},
            "owner_id": {"type": "string", "description": "modify_track optional: target actor id; omit for the acting PC"},
            "track_id": {"type": "string", "description": "modify_track: the kernel resource-track id being changed"},
            "op": {"type": "string", "enum": ["add", "subtract", "set"], "description": "modify_track"},
            "amount": {"type": "integer", "description": "modify_track/start_countdown: integer amount"},
            "object_id": {"type": "string", "description": "set_object_state"},
            "patch": {"type": "object", "description": "set_object_state: mechanical state patch"},
            "label": {"type": "string", "description": "start_countdown"},
            "scale": {"type": "string", "enum": ["instant", "combat_round", "scene_beat", "exploration", "travel", "downtime", "flashback"], "description": "start_countdown"},
            "payload": {"type": "object", "description": "start_countdown: what happens when it fires"}
        },
        "required": ["kind"]});
    json!({"type": "array",
        "description": "Structured checks EXPLICITLY written in the scene text (skill/difficulty/consequence). Empty array when the text states none.",
        "items": {"type": "object", "properties": {
            "intent_id": {"type": "string", "description": "snake_case unique id"},
            "description": {"type": "string", "description": "which player action triggers this check"},
            "tested_parameter": {"type": "string", "description": "the skill/stat the text says is tested"},
            "difficulty": {"type": ["object", "string"],
                "description": "STRUCTURED target object {kind,value} whenever the text gives a number; put conditional variants/original wording in `note`, never inside value. Only when the text truly gives no number, fall back to the verbatim string — a string does NOT bind a settlement target (engine fail-closed to kernel defaults).",
                "properties": {
                    "kind": {"type": "string", "enum": ["dv", "static", "target_number"]},
                    "value": {"type": "integer", "description": "the numeric target"},
                    "note": {"type": "string", "description": "optional: original wording / conditional variants"}
                },
                "required": ["kind", "value"]},
            "effect_policy": {"type": "object", "properties": {
                "on_success": {"type": "array", "items": effect_items.clone()},
                "on_failure": {"type": "array", "items": effect_items}
            }},
            "source_anchor": {"type": "string", "description": "verbatim snippet from the page proving this check exists"}
        }, "required": ["intent_id", "description", "tested_parameter", "source_anchor"]}})
}

/// 从深抽提交的 scene 对象解析 scene_mechanics。fail-closed 三重过滤（逐条独立丢弃，
/// 绝不因一条坏整批废）：① 整条 JSON 反序列化失败 → 丢；② source_anchor 空/全空白 → 丢
/// （不编造——对标 links 的 anchor 收口先例，module_reader_loop.rs）；
/// ③ tested_parameter / intent_id 空 → 丢。按 intent_id 去重（保首条）。
pub(crate) fn parse_scene_mechanics(scene: &Value) -> Vec<SceneMechanicIntent> {
    let Some(arr) = scene.get("scene_mechanics").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<SceneMechanicIntent> = Vec::new();
    for raw in arr {
        // ① 整条反序列化失败 → 丢（逐条独立，不连坐）。
        let Ok(intent) = serde_json::from_value::<SceneMechanicIntent>(raw.clone()) else {
            continue;
        };
        // ② source_anchor 空/全空白 → 丢（不编造）。
        if intent.source_anchor.trim().is_empty() {
            continue;
        }
        // ③ tested_parameter / intent_id 空 → 丢。
        if intent.tested_parameter.trim().is_empty() || intent.intent_id.trim().is_empty() {
            continue;
        }
        // 按 intent_id 去重（保首条）。
        if out.iter().any(|i| i.intent_id == intent.intent_id) {
            continue;
        }
        out.push(intent);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::EffectPatchIntent;

    #[test]
    fn schema_shaped_entries_deserialize_to_executable_variants() {
        // schema/DEEP_SYS 宣称的形态必须与 trpg-model EffectPatchIntent serde 逐字对齐，
        // 否则下游 fold_unexecutable 折成不可执行事实（C7 病灶②的回归护栏）。
        let scene = scene_with(json!([
            {"intent_id": "m1", "description": "强行突入", "tested_parameter": "brawling",
             "difficulty": {"kind": "dv", "value": 13, "note": "原文条件变体"},
             "effect_policy": {"on_success": [
                {"kind": "create_fact", "target": "scene.guards", "fact": {"alerted": false}},
                {"kind": "set_object_state", "object_id": "obj.door", "patch": {"open": true}}
             ], "on_failure": [
                {"kind": "modify_track", "owner_kind": "actor", "track_id": "hp", "op": "subtract", "amount": 2},
                {"kind": "start_countdown", "label": "增援抵达", "amount": 3, "scale": "combat_round", "payload": {}}
             ]},
             "source_anchor": "原文片段"}
        ]));
        let got = parse_scene_mechanics(&scene);
        assert_eq!(got.len(), 1);
        // difficulty 结构对象原样保留（note 等附加字段不破坏 difficulty_to_target 读 kind/value）。
        assert_eq!(got[0].difficulty.as_ref().unwrap()["kind"], "dv");
        assert_eq!(got[0].difficulty.as_ref().unwrap()["value"], 13);
        let ok = &got[0].effect_policy.on_success;
        assert!(matches!(ok[0], EffectPatchIntent::CreateFact { .. }), "create_fact 必须落 CreateFact: {:?}", ok[0]);
        assert!(matches!(ok[1], EffectPatchIntent::SetObjectState { .. }), "set_object_state 必须落 SetObjectState: {:?}", ok[1]);
        let fail = &got[0].effect_policy.on_failure;
        assert!(matches!(fail[0], EffectPatchIntent::ModifyTrack { .. }), "modify_track 必须落 ModifyTrack: {:?}", fail[0]);
        assert!(matches!(fail[1], EffectPatchIntent::StartCountdown { .. }), "start_countdown 必须落 StartCountdown: {:?}", fail[1]);
    }

    #[test]
    fn legacy_value_shaped_create_fact_falls_to_other() {
        // C7 病灶②原形态：{"kind":"create_fact","value":"..."} 缺 target/fact → untagged Other
        //（不可执行）。schema/prompt 防的就是它——形态修好后此形不应再产出，但解析层保持容忍。
        let scene = scene_with(json!([
            {"intent_id": "m1", "description": "旧病灶", "tested_parameter": "brawling",
             "effect_policy": {"on_success": [{"kind": "create_fact", "value": "营救成功"}], "on_failure": []},
             "source_anchor": "原文片段"}
        ]));
        let got = parse_scene_mechanics(&scene);
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0].effect_policy.on_success[0], EffectPatchIntent::Other(_)));
    }

    #[test]
    fn schema_advertises_structured_difficulty_and_exact_effect_kinds() {
        // schema 单一事实源自检：difficulty kind 枚举与 scene_policy::difficulty_to_target
        // 认的三种一致；effect kind 枚举恰为四个可执行变体的 serde tag。
        let s = scene_mechanics_schema();
        let dk = s.pointer("/items/properties/difficulty/properties/kind/enum").unwrap();
        assert_eq!(dk, &json!(["dv", "static", "target_number"]));
        let ek = s
            .pointer("/items/properties/effect_policy/properties/on_success/items/properties/kind/enum")
            .unwrap();
        assert_eq!(ek, &json!(["set_object_state", "modify_track", "create_fact", "start_countdown"]));
    }

    fn scene_with(mechanics: Value) -> Value {
        json!({ "read_aloud": "你们看到一座加油站。", "scene_mechanics": mechanics })
    }

    #[test]
    fn parse_filters_missing_anchor_and_empty_param() {
        // 3 条输入：1 全合法 / 1 无 source_anchor（空串）/ 1 tested_parameter 空串。
        let scene = scene_with(json!([
            {"intent_id": "m1", "description": "强行剪断缆绳", "tested_parameter": "brawling",
             "difficulty": {"kind": "dv", "value": 13}, "source_anchor": "强行剪断缆绳需要检定"},
            {"intent_id": "m2", "description": "无锚编造", "tested_parameter": "stealth",
             "source_anchor": ""},
            {"intent_id": "m3", "description": "无参数", "tested_parameter": "",
             "source_anchor": "原文片段"}
        ]));
        let got = parse_scene_mechanics(&scene);
        assert_eq!(got.len(), 1, "无 anchor 与空 tested_parameter 各被丢弃");
        assert_eq!(got[0].intent_id, "m1");
        assert_eq!(got[0].description, "强行剪断缆绳");
        assert_eq!(got[0].tested_parameter, "brawling");
        assert_eq!(got[0].difficulty, Some(json!({"kind": "dv", "value": 13})));
        assert_eq!(got[0].source_anchor, "强行剪断缆绳需要检定");
    }

    #[test]
    fn parse_dedups_by_intent_id_keeps_first() {
        let scene = scene_with(json!([
            {"intent_id": "m1", "description": "首条", "tested_parameter": "brawling",
             "source_anchor": "原文甲"},
            {"intent_id": "m1", "description": "重复条", "tested_parameter": "athletics",
             "source_anchor": "原文乙"}
        ]));
        let got = parse_scene_mechanics(&scene);
        assert_eq!(got.len(), 1, "同 intent_id 去重");
        assert_eq!(got[0].description, "首条", "保首条");
    }

    #[test]
    fn parse_tolerates_malformed_entry() {
        // 数组里混一个非对象（字符串）→ 其余正常解析、不 panic。
        let scene = scene_with(json!([
            "这不是一个对象",
            {"intent_id": "m1", "description": "合法条", "tested_parameter": "brawling",
             "source_anchor": "原文片段"}
        ]));
        let got = parse_scene_mechanics(&scene);
        assert_eq!(got.len(), 1, "坏条独立丢弃，不连坐");
        assert_eq!(got[0].intent_id, "m1");
    }

    #[test]
    fn parse_missing_key_returns_empty() {
        // scene 无 scene_mechanics 键 / 键为 null → 空 vec。
        let no_key = json!({"read_aloud": "你们看到一座加油站。"});
        assert!(parse_scene_mechanics(&no_key).is_empty());
        let null_key = json!({"scene_mechanics": null});
        assert!(parse_scene_mechanics(&null_key).is_empty());
    }
}
