//! 桌面骰权政策(TRPG_AGENT_TABLE_DICE_POLICY)的单一事实源谓词。
//! 各 crate 一律委托此函数判定,不得复制 env 解析逻辑。

/// 桌面骰权政策是否允许系统当场代掷并公开结果。
/// 未设置时默认 true(system_rolls_visible)。
pub fn system_rolls_visible_policy() -> bool {
    let v = std::env::var("TRPG_AGENT_TABLE_DICE_POLICY").unwrap_or_else(|_| "system_rolls_visible".into());
    matches!(v.to_ascii_lowercase().as_str(), "system_rolls_visible" | "system" | "gm_rolls_visible" | "auto" | "auto_visible")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// env var 是进程级状态,全部分支在单测试函数内串行验证,结束后恢复原值。
    #[test]
    fn policy_value_list_and_default() {
        let key = "TRPG_AGENT_TABLE_DICE_POLICY";
        let prior = std::env::var(key).ok();

        std::env::remove_var(key);
        assert!(system_rolls_visible_policy(), "unset must default to true");

        for v in ["system_rolls_visible", "system", "gm_rolls_visible", "auto", "auto_visible", "AUTO", "System_Rolls_Visible"] {
            std::env::set_var(key, v);
            assert!(system_rolls_visible_policy(), "{v} must be visible");
        }
        for v in ["player_rolls", "manual", "off", ""] {
            std::env::set_var(key, v);
            assert!(!system_rolls_visible_policy(), "{v} must not be visible");
        }

        match prior {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
