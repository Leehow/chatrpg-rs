//! 物件/法术 schema 懒编译政策(TRPG_LAZY_OBJECT_SCHEMA)的单一事实源谓词。
//! Stage-2 编译门 与 play-time 激活钩子 一律委托此函数,不得复制 env 解析逻辑。

/// 是否启用"发现即存根、首次引用再编译"的懒物件/法术 schema 路径。
///
/// 默认 **关**:Stage 2 仍做 eager 全量编译(discover + 逐类抽取),schema 在开局
/// 前就绪、回合内无 LLM 卡顿——这是除法术海量规则集外的安全默认。对 D&D 这类
/// 数百法术、eager 全量编译过慢的规则集,设 `TRPG_LAZY_OBJECT_SCHEMA=1` 开启:
/// Stage 2 只发现存根,逐类 schema 推迟到 play 时首次引用再补
/// (`MaterializationService::ensure_category_compiled`),并缓存回 kernel。
pub fn lazy_object_schema_enabled() -> bool {
    lazy_flag_from(std::env::var("TRPG_LAZY_OBJECT_SCHEMA").ok())
}

/// 纯解析:未设置 / 不识别的值 => 关。拆出来让"默认关 + 接受值"契约可在
/// **不改写进程级 env** 的前提下单测——并发 `set_var`/`getenv` 在 glibc 上是数据竞争
/// (本 crate 测试套件内多处 env 测试并行会触发),故新测试一律走纯函数。
fn lazy_flag_from(raw: Option<String>) -> bool {
    raw.map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lazy_flag_default_off_and_accepted_values() {
        // 未设置(None):默认关——eager 全量编译仍是默认,开局前 schema 就绪、无回合内卡顿。
        assert!(
            !lazy_flag_from(None),
            "unset must default to OFF (eager stays default)"
        );
        // 显式打开(法术海量规则集如 D&D)。
        for v in ["1", "true", "yes", "on", "ON", "True"] {
            assert!(lazy_flag_from(Some(v.to_string())), "{v} must enable lazy");
        }
        // 关闭族 + 任意杂值 -> 关。
        for v in ["0", "false", "off", "no", "", "garbage"] {
            assert!(
                !lazy_flag_from(Some(v.to_string())),
                "{v} must keep lazy OFF"
            );
        }
    }
}
