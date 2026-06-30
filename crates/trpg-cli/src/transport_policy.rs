// crates/trpg-cli/src/transport_policy.rs
//
// R5 T3：CLI transport 等待策略——纯函数，零 IO，单测友好。
// `trpg turn`（一次性）需确定性落账：等 heavy complete 再退进程。
// `trpg play`（交互）不等：TurnComplete 后即开始下一回合；下一轮
//   `await_prev_turn_critical` 高水位守卫保证正确性。

/// CLI transport 决策：一次性 turn 需等 heavy 落账，交互 play 不等（由下一轮守卫兜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitMode {
    /// 等 heavy 完成（`pp_lifecycle = complete`）再退出进程。
    WaitHeavy,
    /// 不等：TurnComplete 后即可开始下一回合（high-water guard 兜）。
    NoWait,
}

/// 根据 CLI 使用场景选择 `WaitMode`。
/// `is_one_shot` = true  → `trpg turn`（脚本/测试，需确定性落账）。
/// `is_one_shot` = false → `trpg play`（交互循环，下一轮守卫兜）。
pub fn cli_wait_mode(is_one_shot: bool) -> WaitMode {
    let override_value = std::env::var("TRPG_TURN_WAIT_HEAVY").ok();
    cli_wait_mode_with_override(is_one_shot, override_value.as_deref())
}

pub fn cli_wait_mode_with_override(
    is_one_shot: bool,
    wait_heavy_override: Option<&str>,
) -> WaitMode {
    if !is_one_shot {
        return WaitMode::NoWait;
    }
    if let Some(value) = wait_heavy_override {
        let normalized = value.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "0" | "false" | "no" | "off") {
            return WaitMode::NoWait;
        }
        if matches!(normalized.as_str(), "1" | "true" | "yes" | "on") {
            return WaitMode::WaitHeavy;
        }
    }
    if is_one_shot {
        WaitMode::WaitHeavy
    } else {
        WaitMode::NoWait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_turn_waits_heavy() {
        assert_eq!(cli_wait_mode(true), WaitMode::WaitHeavy);
    }

    #[test]
    fn interactive_play_no_wait() {
        assert_eq!(cli_wait_mode(false), WaitMode::NoWait);
    }

    #[test]
    fn wait_heavy_ne_no_wait() {
        assert_ne!(WaitMode::WaitHeavy, WaitMode::NoWait);
    }

    #[test]
    fn wait_mode_debug_str_non_empty() {
        // WaitMode 实现 Debug，用于 tracing 日志。
        assert!(!format!("{:?}", WaitMode::WaitHeavy).is_empty());
        assert!(!format!("{:?}", WaitMode::NoWait).is_empty());
    }

    #[test]
    fn cli_wait_mode_is_pure_deterministic() {
        // 同参数反复调用返回同值（纯函数）。
        for _ in 0..5 {
            assert_eq!(cli_wait_mode_with_override(true, None), WaitMode::WaitHeavy);
            assert_eq!(cli_wait_mode_with_override(false, None), WaitMode::NoWait);
        }
    }

    #[test]
    fn one_shot_turn_can_disable_heavy_wait_for_eval() {
        assert_eq!(
            cli_wait_mode_with_override(true, Some("false")),
            WaitMode::NoWait
        );
        assert_eq!(
            cli_wait_mode_with_override(true, Some("0")),
            WaitMode::NoWait
        );
    }

    #[test]
    fn interactive_play_ignores_wait_heavy_override() {
        assert_eq!(
            cli_wait_mode_with_override(false, Some("true")),
            WaitMode::NoWait
        );
    }
}
