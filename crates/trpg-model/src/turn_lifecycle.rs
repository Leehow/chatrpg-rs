//! 回合后处理（R5 postprocess）生命周期状态机常量与秩函数。
//!
//! 从 `lib.rs` facade 拆出的低耦合定义。通过 `pub use turn_lifecycle::*`
//! 在 crate 根重导出，公共 API 保持不变。

// R5 postprocess 生命周期状态机（turns.pp_lifecycle 列的取值，集中常量）：
// streaming → critical_done → complete。与 turns.postprocess_status（ready/awaiting）
// 正交：后者是 finalize 终态，前者是回合后处理的临界/重活落账进度（高水位守卫据此）。
pub const PP_STREAMING: &str = "streaming";
pub const PP_CRITICAL_DONE: &str = "critical_done";
pub const PP_COMPLETE: &str = "complete";

/// 生命周期阶段的有序秩（守卫用：>= critical_done 即可继续，未知值排最低 fail-closed）。
pub fn pp_lifecycle_rank(phase: &str) -> u8 {
    match phase {
        PP_STREAMING => 1,
        PP_CRITICAL_DONE => 2,
        PP_COMPLETE => 3,
        _ => 0, // 未知/旧值/空 → 最低，守卫视作未达 critical（fail-closed 多等不误读）
    }
}
