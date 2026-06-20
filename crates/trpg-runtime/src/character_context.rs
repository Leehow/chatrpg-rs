//! OA2 (G-3) — player-safe PC 能力载体。
//!
//! 分体 Narrator(`TRPG_NARRATOR_SPLIT`)的 `NarrationPacket` 此前不含任何角色卡数据
//! (CoC 45+ 技能 / 8 属性、Cyberpunk 19 技能 / 10 属性都在 DB 但永不进 Narrator)→ 念白
//! 无法引用 PC 的真实能力 / 特长。本模块把玩家**自己的**角色卡数值能力(stats/skills/…)渲染成
//! 紧凑的 player-safe 参考行,经 `CompiledContext.character_context` 喂给分体 Narrator。
//!
//! # player-safe
//! 角色卡是玩家自己的角色,**本就玩家可知** ⇒ 渲染它**不新增任何泄漏面**。仍做纵深防御:
//! 只读数值能力桶、跳过 GM-only/hidden 键、跳过派生噪声键(`*_half`/`*_fifth`/`*_double`)。
//!
//! # 加性 / 字节级基线
//! 仅 `Enforce` 下由 `prepare_turn_context` 填充;Off/Shadow ⇒ 空 ⇒ 分体 Narrator 注入空 ⇒
//! OFF 字节等价基线。纯函数无副作用、不落库。零 ruleset/module 名分支(全按桶名 data-driven)。

use serde_json::Value;

/// 玩家可知的数值能力桶(语义字段名,data-driven,非 ruleset 名分支)。
const COMPETENCY_BUCKETS: &[&str] = &["stats", "skills", "fields", "attributes"];

fn key_is_gm_only(key: &str) -> bool {
    // Compound markers only — bare words like "hidden"/"secret" collide with legitimate skill
    // names (CoC "Spot Hidden"). Primary protection is the bucket whitelist; this is
    // defense-in-depth against an explicitly GM-tagged key inside a competency bucket.
    let k = key.to_ascii_lowercase();
    ["gm_only", "keeper_only", "_secret", "secret_", "spoiler"]
        .iter()
        .any(|m| k.contains(m))
}

/// 派生 / 半值噪声键(CoC 的 `con_half`/`app_fifth` 等)——机械中间量,非念白可引用的能力。
fn key_is_derived_noise(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.ends_with("_half") || k.ends_with("_fifth") || k.ends_with("_double")
}

/// 把一个值解析成可读数值(直接数字,或 `{value|rating|current}` 包裹)。
fn numeric_label(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::Object(o) => {
            for k in ["value", "rating", "current", "score"] {
                if let Some(n) = o.get(k).and_then(Value::as_i64) {
                    return Some(n.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

/// 渲染 PC 的 player-safe 能力档案(每桶一行,确定性排序)。空 sheet ⇒ 空 vec。
pub fn collect_character_context(sheet_json: &Value, display_name: Option<&str>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for bucket in COMPETENCY_BUCKETS {
        let Some(obj) = sheet_json.get(bucket).and_then(Value::as_object) else {
            continue;
        };
        let mut entries: Vec<(String, String)> = Vec::new();
        for (k, v) in obj {
            if key_is_gm_only(k) || key_is_derived_noise(k) {
                continue;
            }
            if let Some(num) = numeric_label(v) {
                entries.push((k.clone(), num));
            }
        }
        if entries.is_empty() {
            continue;
        }
        entries.sort();
        let rendered: Vec<String> = entries.iter().map(|(k, n)| format!("{k} {n}")).collect();
        lines.push(format!("{bucket}: {}", rendered.join("、")));
    }
    if lines.is_empty() {
        return Vec::new();
    }
    let name = display_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("调查员");
    let mut out = vec![format!("【{name} 的能力档案(玩家自知,供念白引用,勿逐字罗列)】")];
    out.extend(lines);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_skills_and_stats_player_safe() {
        let sheet = json!({
            "stats": {"STR": 55, "DEX": 70, "INT": 75, "con_half": 30, "app_fifth": 12},
            "skills": {"Listen": 50, "Persuade": 45, "Spot Hidden": 50, "keeper_only_truth": 99},
            "anomaly_gm_only": {"truth": 99}
        });
        let out = collect_character_context(&sheet, Some("Evelyn Price"));
        let joined = out.join("\n");
        assert!(joined.contains("Evelyn Price"), "{joined}");
        assert!(joined.contains("STR 55") && joined.contains("DEX 70"), "{joined}");
        // "Spot Hidden" is a legitimate player-visible CoC skill — it must NOT be filtered
        // just because its name contains the substring "hidden".
        assert!(joined.contains("Persuade 45") && joined.contains("Spot Hidden 50"), "{joined}");
        // 派生噪声 & GM-only 桶 & 桶内 GM-only 键绝不出现。
        assert!(!joined.contains("con_half"), "derived noise leaked: {joined}");
        assert!(!joined.contains("anomaly"), "gm_only bucket leaked: {joined}");
        assert!(!joined.contains("keeper_only"), "in-bucket gm_only key leaked: {joined}");
    }

    #[test]
    fn empty_sheet_yields_empty() {
        assert!(collect_character_context(&json!({}), Some("x")).is_empty());
        assert!(collect_character_context(&json!({"stats": {}}), None).is_empty());
    }

    #[test]
    fn nested_value_objects_supported() {
        let sheet = json!({"skills": {"Hacking": {"value": 14}, "Brawl": {"rating": 9}}});
        let out = collect_character_context(&sheet, None).join("\n");
        assert!(out.contains("Hacking 14"), "{out}");
        assert!(out.contains("Brawl 9"), "{out}");
    }
}
