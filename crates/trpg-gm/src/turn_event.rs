//! 统一回合事件流（R1）：execute_turn 把过去 GmLoop 的 on_delta 回调 + TurnOutcome 返回
//! 收编成单一事件流。transport（CLI 同步 drain / API spawn drain）各自解释这些事件。

use crate::errata::ErrataEntry;
use crate::turn_loop::TurnOutcome;

/// 一个回合在执行过程中向 transport 发出的事件。
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// 逐 token 真流式叙事增量（直通不缓冲）。
    Delta(String),
    /// 桌面骰 gate：玩家须手摇，回合在此早返。
    AwaitingPlayerRoll { check_id: String, prompt_public: String },
    /// scene_navigate 产出的场景切换。
    SceneTransition { from: String, to: String, reason: String },
    /// 后置勘误条目（不阻塞叙事，注入下一轮上下文）。
    Errata(ErrataEntry),
    /// 叙事完成、尾部 phase 开始——transport 据此决定前台/后台执行尾部。
    PostprocessScheduled,
    /// heavy 组后台任务完成信号（CLI turn 模式等此事件或轮询 pp_lifecycle=complete；
    /// API SSE / play 模式已在 TurnComplete 处 break，不等此事件）。
    HeavyPostprocessDone,
    /// 回合终态。
    TurnComplete { outcome: TurnOutcome },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errata::ErrataEntry;
    use crate::turn_loop::TurnOutcome;
    use chrono::Utc;
    use trpg_agent::VerifierFindingKind;

    #[test]
    fn all_variants_construct_and_clone_and_debug() {
        let errata = ErrataEntry {
            kind: VerifierFindingKind::OmittedVisibleResult,
            detail: "x".into(),
            turn_id: "t1".into(),
            created_at: Utc::now(),
        };
        let events = vec![
            TurnEvent::Delta("hi".into()),
            TurnEvent::AwaitingPlayerRoll { check_id: "c1".into(), prompt_public: "roll".into() },
            TurnEvent::SceneTransition { from: "a".into(), to: "b".into(), reason: "moved".into() },
            TurnEvent::Errata(errata),
            TurnEvent::PostprocessScheduled,
            TurnEvent::HeavyPostprocessDone,
            TurnEvent::TurnComplete { outcome: TurnOutcome::Narration("done".into()) },
        ];
        for e in &events {
            let cloned = e.clone();
            assert!(!format!("{cloned:?}").is_empty());
        }
        assert_eq!(events.len(), 7);
    }

    #[test]
    fn turn_complete_carries_awaiting_outcome() {
        let e = TurnEvent::TurnComplete {
            outcome: TurnOutcome::AwaitingPlayerRoll { check_id: "c1".into(), prompt_public: "roll".into() },
        };
        match e {
            TurnEvent::TurnComplete { outcome: TurnOutcome::AwaitingPlayerRoll { check_id, .. } } => {
                assert_eq!(check_id, "c1");
            }
            _ => panic!("expected TurnComplete with AwaitingPlayerRoll"),
        }
    }
}
