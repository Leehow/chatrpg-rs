//! 反剧透实体元数据（DATA 半：抽取期打标 + 投影期裁剪的契约类型）。
//!
//! 模组实体（NPC/线索）或场景节点可携带 `SpoilerMeta`：
//! - `secret_terms`：揭示前必须从 GM context 里抹掉的剧透词/短语（如真实身份、凶手名）。
//! - `public_aliases`：玩家此刻已知的安全称呼（揭示前 GM 用它指代该实体）。
//! - `reveal_conditions`：何时向玩家透露的条件（描述性，GM 判定用；**引擎绝不**做
//!   关键词匹配自动揭示——揭示由 revealed-facts 账本显式落账驱动，零规则集硬编码）。
//!
//! 守理念：三字段全 `#[serde(default)]` 向后兼容（旧 bundle 无此键照常 load）；
//! 裁剪只对**显式标了 secret_terms 且未揭示**的实体生效（别太严：没标剧透/已揭示
//! 一律原样过，绝不裁可玩内容）。
//!
//! 本类型只表示 + 提供纯函数原语（redact/safe_name/from_value），**不**碰 DB、不评估
//! reveal_conditions——账本读写在 trpg-db、投影裁剪在 trpg-runtime/spoiler_guard。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 裁剪占位符：secret_term 命中处替换成它（而非整段删除），保留可读性/可玩性。
pub const SPOILER_REDACTION_PLACEHOLDER: &str = "[未揭示]";

/// 单个实体/场景的反剧透元数据。三字段全 serde default，旧数据零负担。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub struct SpoilerMeta {
    /// 揭示前必须抹掉的剧透词/短语。
    #[serde(default)]
    pub secret_terms: Vec<String>,
    /// 玩家已知的安全称呼（揭示前 GM 用以指代）。
    #[serde(default)]
    pub public_aliases: Vec<String>,
    /// 何时揭示的条件（描述性，GM 判定；引擎不自动评估）。
    #[serde(default)]
    pub reveal_conditions: Vec<String>,
}

impl SpoilerMeta {
    /// 三字段（去空白后）全空 → 视为无剧透元数据。`skip_serializing_if` 用它保持
    /// 旧数据（空 spoiler）序列化字节不变；裁剪侧用它做"别太严"的快速放行判定。
    pub fn is_empty(&self) -> bool {
        let blank = |v: &Vec<String>| v.iter().all(|s| s.trim().is_empty());
        blank(&self.secret_terms) && blank(&self.public_aliases) && blank(&self.reveal_conditions)
    }

    /// 从任意实体 JSON 读 SpoilerMeta：优先嵌套 `spoiler` 对象，回退根级扁平字段
    /// （secret_terms/public_aliases/reveal_conditions）。两种抽取形态都吃，fail-soft
    /// 解析失败 → 空（不裁剪，别太严）。
    pub fn from_value(v: &serde_json::Value) -> Self {
        if let Some(nested) = v.get("spoiler") {
            if let Ok(m) = serde_json::from_value::<SpoilerMeta>(nested.clone()) {
                return m;
            }
        }
        // 扁平回退：实体根对象里直接带三字段（SpoilerMeta 无 deny_unknown_fields，
        // 会忽略实体其余键，只挑这三个）。
        serde_json::from_value::<SpoilerMeta>(v.clone()).unwrap_or_default()
    }

    /// 把 text 里每个 secret_term 的出现替换为占位符（子串精确匹配，区分大小写）。
    /// 无 secret_terms → 原样返回（零拷贝语义上不变）。空白 term 跳过（防全文误伤）。
    pub fn redact(&self, text: &str) -> String {
        redact_secret_terms(text, &self.secret_terms)
    }

    /// 揭示前用于指代该实体的安全显示名：首个非空 public_alias；没有别名则回退原名
    /// （原名若本身是 secret_term，会在 redact 阶段被占位符盖掉，故此处不强行兜底）。
    pub fn safe_name<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.public_aliases
            .iter()
            .map(|s| s.as_str())
            .find(|s| !s.trim().is_empty())
            .unwrap_or(fallback)
    }
}

/// 纯函数：把 text 里每个 term（非空白）的出现替换为占位符。供 SpoilerMeta::redact 与
/// 跨实体合并裁剪共用。
pub fn redact_secret_terms(text: &str, terms: &[String]) -> String {
    let mut out = text.to_string();
    for term in terms {
        let t = term.trim();
        if t.is_empty() {
            continue;
        }
        if out.contains(t) {
            out = out.replace(t, SPOILER_REDACTION_PLACEHOLDER);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_spoiler_is_empty_and_skips_serialization_shape() {
        let m = SpoilerMeta::default();
        assert!(m.is_empty(), "默认空 → is_empty");
        // 全空白也算空（别太严：脏空白不触发裁剪）。
        let blanky = SpoilerMeta {
            secret_terms: vec!["  ".into()],
            public_aliases: vec![],
            reveal_conditions: vec!["".into()],
        };
        assert!(blanky.is_empty(), "全空白字段 → is_empty");
        let real = SpoilerMeta {
            secret_terms: vec!["凶手".into()],
            ..Default::default()
        };
        assert!(!real.is_empty(), "有实词 → 非空");
    }

    #[test]
    fn roundtrip_serialize_deserialize() {
        let m = SpoilerMeta {
            secret_terms: vec!["真凶莫里亚蒂".into(), "邪教首领".into()],
            public_aliases: vec!["管家詹姆斯".into()],
            reveal_conditions: vec!["玩家在地窖发现日记后".into()],
        };
        let v = serde_json::to_value(&m).unwrap();
        let back: SpoilerMeta = serde_json::from_value(v).unwrap();
        assert_eq!(back, m, "SpoilerMeta 必 round-trip");
    }

    #[test]
    fn back_compat_missing_fields_default_empty() {
        // 旧载荷无任何 spoiler 字段 → 全 default 空 vec，反序列化绝不失败。
        let raw = json!({});
        let m: SpoilerMeta = serde_json::from_value(raw).unwrap();
        assert!(m.is_empty());
        // 只给部分字段也行。
        let partial: SpoilerMeta = serde_json::from_value(json!({"secret_terms": ["x"]})).unwrap();
        assert_eq!(partial.secret_terms, vec!["x".to_string()]);
        assert!(partial.public_aliases.is_empty());
    }

    #[test]
    fn from_value_reads_nested_then_flat() {
        // 嵌套 spoiler 对象形态。
        let nested = json!({
            "id": "npc_butler", "name": "詹姆斯",
            "spoiler": {"secret_terms": ["凶手"], "public_aliases": ["管家"]}
        });
        let m = SpoilerMeta::from_value(&nested);
        assert_eq!(m.secret_terms, vec!["凶手".to_string()]);
        assert_eq!(m.public_aliases, vec!["管家".to_string()]);
        // 扁平形态（实体根直接带三字段）。
        let flat = json!({
            "id": "npc_butler", "name": "詹姆斯",
            "secret_terms": ["凶手"], "public_aliases": ["管家"], "body": "无关正文"
        });
        let m2 = SpoilerMeta::from_value(&flat);
        assert_eq!(m2.secret_terms, vec!["凶手".to_string()]);
        // 无任何 spoiler 信息 → 空（别太严）。
        let none = json!({"id": "npc_x", "name": "路人", "body": "他是个普通人。"});
        assert!(SpoilerMeta::from_value(&none).is_empty());
    }

    #[test]
    fn redact_replaces_each_secret_term_with_placeholder() {
        let m = SpoilerMeta {
            secret_terms: vec!["连环杀手".into(), "莫里亚蒂教授".into()],
            ..Default::default()
        };
        let red = m.redact("他其实是连环杀手，真名莫里亚蒂教授。");
        assert!(!red.contains("连环杀手"), "secret_term 必被抹: {red}");
        assert!(!red.contains("莫里亚蒂教授"), "secret_term 必被抹: {red}");
        assert!(
            red.contains(SPOILER_REDACTION_PLACEHOLDER),
            "应留占位符: {red}"
        );
        // 非剧透内容保留（别太严：只动标了的词）。
        assert!(red.contains("他其实是"), "非剧透文本保留: {red}");
    }

    #[test]
    fn redact_noop_when_no_secret_terms() {
        let m = SpoilerMeta {
            public_aliases: vec!["管家".into()],
            ..Default::default()
        };
        let text = "他是个普通的管家。";
        assert_eq!(
            m.redact(text),
            text,
            "无 secret_terms → 原样（不裁可玩内容）"
        );
    }

    #[test]
    fn redact_skips_blank_terms_to_avoid_global_wipe() {
        let m = SpoilerMeta {
            secret_terms: vec!["".into(), "  ".into()],
            ..Default::default()
        };
        let text = "完整的可玩文本不该被空白 term 误伤。";
        assert_eq!(m.redact(text), text, "空白 term 必跳过，绝不全文误伤");
    }

    #[test]
    fn safe_name_prefers_alias_else_fallback() {
        let with_alias = SpoilerMeta {
            public_aliases: vec!["  ".into(), "管家詹姆斯".into()],
            ..Default::default()
        };
        assert_eq!(
            with_alias.safe_name("莫里亚蒂"),
            "管家詹姆斯",
            "首个非空别名优先"
        );
        let no_alias = SpoilerMeta {
            secret_terms: vec!["凶手".into()],
            ..Default::default()
        };
        assert_eq!(no_alias.safe_name("詹姆斯"), "詹姆斯", "无别名 → 回退原名");
    }
}
