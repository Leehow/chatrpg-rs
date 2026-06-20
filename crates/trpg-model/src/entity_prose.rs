//! 实体 body-prose 读取的单一事实源（NPC/clue/location/scene 等模组图谱实体）。
//!
//! 背景：模组深抽器 SYS prompt 让 LLM 为每个实体输出 `name + 正文`，模型**非确定性**
//! 地有时用英文键 `body`、有时用中文键 `正文`（同一 bundle 内不同 NPC 可能各异）。
//! 历史读站点只查 `body`→`summary`，于是 `正文`-键实体的人设/简介读成空字符串
//! （证据见 CAPABILITY_AUDIT：Cyberpunk 9 个 NPC 里 5 个含 npc_athena 用 `正文`）。
//!
//! 本 helper 把这个选择收敛到一处，供 trpg-runtime / trpg-gm / trpg-parser 各读站点复用。
//!
//! ## 行为保持（behavior-preserving）
//! env flag `TRPG_NPC_BODY_KEY_NORMALIZE` 控制 `正文` 回退：
//! - **默认 OFF** ⇒ 与历史 idiom `v.get("body").or_else(|| v.get("summary"))` **逐字节等价**
//!   （`正文`-键实体仍读空，旧 persona/scene prose 字节不变）。
//! - **ON** ⇒ 顺序 `body` → `正文` → `summary`，`正文`-键实体获得其 prose。
//!
//! ## 通用性（constitution ⑪ GENERIC）
//! 零 ruleset / 模组专名分支：只按 JSON 键选择，对任意模组一视同仁。
//!
//! ## OFF 字节等价的关键不变量（codex 设计复核 ③ 折入）
//! 必须**先按键选 `Value` 再 `as_str`**，等价于
//! `v.get("body").or_else(|| v.get("summary")).and_then(Value::as_str)`。
//! 切勿写成"每键各自 `as_str` 后 or_else"——否则 `body` 键存在但非字符串时会错误回退到
//! `summary`，破坏旧语义。

use serde_json::Value;

/// 环境变量名：开启 `正文` 中文别名键回退。默认 OFF。
pub const NPC_BODY_KEY_NORMALIZE_ENV: &str = "TRPG_NPC_BODY_KEY_NORMALIZE";

/// `正文` 回退是否开启（读 env，默认 OFF）。
/// `0/false/off/no`（大小写不敏感）→ OFF；其余非空真值 → ON；未设置 → OFF。
pub fn body_key_normalize_enabled() -> bool {
    std::env::var(NPC_BODY_KEY_NORMALIZE_ENV)
        .map(|v| {
            matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
}

/// 选出实体 body-prose 字段的 `Value`（未做 `as_str`），保留原始类型供调用方决定。
/// OFF：`body` → `summary`。ON：`body` → `正文` → `summary`。
/// 先选 `Value` 再由调用方/`entity_body_prose` 做 `as_str`，保证 OFF 字节等价。
pub fn entity_body_value<'a>(v: &'a Value) -> Option<&'a Value> {
    if body_key_normalize_enabled() {
        v.get("body")
            .or_else(|| v.get("正文"))
            .or_else(|| v.get("summary"))
    } else {
        v.get("body").or_else(|| v.get("summary"))
    }
}

/// 读实体 body-prose 为 `&str`。OFF 与历史 idiom 逐字节等价；ON 增 `正文` 回退。
/// 字段缺失或非字符串 → None（与旧 `.and_then(Value::as_str)` 同语义）。
pub fn entity_body_prose<'a>(v: &'a Value) -> Option<&'a str> {
    entity_body_value(v).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    // env 是进程全局态，串行化避免并行测试互相污染 flag。
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_flag<T>(on: bool, f: impl FnOnce() -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap();
        if on {
            std::env::set_var(NPC_BODY_KEY_NORMALIZE_ENV, "1");
        } else {
            std::env::remove_var(NPC_BODY_KEY_NORMALIZE_ENV);
        }
        let r = f();
        std::env::remove_var(NPC_BODY_KEY_NORMALIZE_ENV);
        r
    }

    #[test]
    fn off_zhengwen_npc_reads_blank_byte_equal_baseline() {
        // 正文-键 NPC（如 npc_athena）：OFF 下读空 = 历史基线字节等价。
        let v = json!({"id": "npc_athena", "name": "雅典娜", "正文": "冷峻的网络幽灵。"});
        with_flag(false, || {
            assert_eq!(entity_body_prose(&v), None);
        });
    }

    #[test]
    fn on_zhengwen_npc_gains_its_prose() {
        let v = json!({"id": "npc_athena", "name": "雅典娜", "正文": "冷峻的网络幽灵。"});
        with_flag(true, || {
            assert_eq!(entity_body_prose(&v), Some("冷峻的网络幽灵。"));
        });
    }

    #[test]
    fn body_keyed_npc_works_both_ways() {
        let v = json!({"id": "npc.lars", "name": "拉斯", "body": "沉默寡言的退伍兵。"});
        with_flag(false, || {
            assert_eq!(entity_body_prose(&v), Some("沉默寡言的退伍兵。"));
        });
        with_flag(true, || {
            assert_eq!(entity_body_prose(&v), Some("沉默寡言的退伍兵。"));
        });
    }

    #[test]
    fn summary_fallback_still_works_both_ways() {
        let v = json!({"id": "npc.x", "name": "X", "summary": "索引简介。"});
        with_flag(false, || {
            assert_eq!(entity_body_prose(&v), Some("索引简介。"));
        });
        with_flag(true, || {
            assert_eq!(entity_body_prose(&v), Some("索引简介。"));
        });
    }

    #[test]
    fn body_wins_over_zhengwen_and_summary_when_on() {
        let v = json!({"body": "B", "正文": "Z", "summary": "S"});
        with_flag(true, || {
            assert_eq!(entity_body_prose(&v), Some("B"));
        });
        // OFF：body 仍优先（正文 不参与）。
        with_flag(false, || {
            assert_eq!(entity_body_prose(&v), Some("B"));
        });
    }

    #[test]
    fn zhengwen_wins_over_summary_when_on() {
        let v = json!({"正文": "Z", "summary": "S"});
        with_flag(true, || {
            assert_eq!(entity_body_prose(&v), Some("Z"));
        });
        // OFF：正文 不参与 → 落到 summary（基线）。
        with_flag(false, || {
            assert_eq!(entity_body_prose(&v), Some("S"));
        });
    }

    #[test]
    fn non_string_body_does_not_fallthrough_to_summary_off_byte_equal() {
        // 关键不变量（codex ③）：body 键存在但非字符串 → 选中 body 的 Value 再 as_str=None，
        // **不**回退到 summary。等价于历史 .or_else(summary).and_then(as_str)。
        let v = json!({"body": 123, "summary": "S"});
        with_flag(false, || {
            assert_eq!(entity_body_prose(&v), None);
        });
        with_flag(true, || {
            // ON 亦然：body 先被选中（非字符串）→ None，不串到 正文/summary。
            assert_eq!(entity_body_prose(&v), None);
        });
    }

    #[test]
    fn flag_off_values_are_false() {
        for raw in ["0", "false", "off", "no", "FALSE", "Off", ""] {
            let _g = ENV_LOCK.lock().unwrap();
            std::env::set_var(NPC_BODY_KEY_NORMALIZE_ENV, raw);
            assert!(!body_key_normalize_enabled(), "{raw:?} 应判 OFF");
            std::env::remove_var(NPC_BODY_KEY_NORMALIZE_ENV);
        }
    }
}
