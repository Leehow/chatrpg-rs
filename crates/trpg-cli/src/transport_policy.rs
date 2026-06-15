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
    if is_one_shot { WaitMode::WaitHeavy } else { WaitMode::NoWait }
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
            assert_eq!(cli_wait_mode(true), WaitMode::WaitHeavy);
            assert_eq!(cli_wait_mode(false), WaitMode::NoWait);
        }
    }
}
