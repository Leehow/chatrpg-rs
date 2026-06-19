//! P2 步骤5：PresentationGate 判定（纯函数，零 IO / 零 LLM）。
//!
//! 把 [`NarrationVerifierResult`] 按 severity+kind 折成 [`PresentationGate`]。
//! Charter §5 P2 已拍板的**锁定**白名单（EXHAUSTIVE）：仅当某 finding 同时满足
//! `severity == Blocker` 且 `kind ∈ {SecretLeak, InventedEffect, ManualRollRequest}` 时
//! 才 `Block`；其余（Warning / MissingCheck / MissingRollExecution / MissingEffect /
//! OmittedVisibleResult 等走 RetroDebt 补账的）一律 `Allow`（保持既有 errata + RetroDebt 路径）。
//!
//! 白名单是锁定测试常量：任何 future widening 都强制改 [`BLOCKING_KINDS`] + 穷举测试
//! （= 显式人审点）。本判定 advisory，不在 OFF 路径产生任何行为变更。

use trpg_agent::{NarrationVerifierResult, VerifierFinding, VerifierFindingKind, VerifierSeverity};

/// PresentationGate 决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationGate {
    /// 允许交付（既有 errata + RetroDebt 路径不变）。
    Allow,
    /// 阻断交付，携带触发阻断的 Blocker findings（供 repair ladder + trace 用）。
    Block(Vec<VerifierFinding>),
}

impl Default for PresentationGate {
    fn default() -> Self {
        PresentationGate::Allow
    }
}

impl PresentationGate {
    pub fn is_block(&self) -> bool {
        matches!(self, PresentationGate::Block(_))
    }
}

/// 锁定的可阻断 kind 白名单（EXHAUSTIVE，charter §5 P2）。任何新增/移除都强制改穷举测试。
pub const BLOCKING_KINDS: [VerifierFindingKind; 3] = [
    VerifierFindingKind::SecretLeak,
    VerifierFindingKind::InventedEffect,
    VerifierFindingKind::ManualRollRequest,
];

/// 单 finding 是否触发阻断：Blocker 且 kind ∈ BLOCKING_KINDS。
fn finding_blocks(f: &VerifierFinding) -> bool {
    f.severity == VerifierSeverity::Blocker && BLOCKING_KINDS.contains(&f.kind)
}

/// 纯判定：收集所有触发阻断的 findings；非空 → Block，否则 Allow。
pub fn presentation_gate_decision(result: &NarrationVerifierResult) -> PresentationGate {
    let blocking: Vec<VerifierFinding> = result
        .findings
        .iter()
        .filter(|f| finding_blocks(f))
        .cloned()
        .collect();
    if blocking.is_empty() {
        PresentationGate::Allow
    } else {
        PresentationGate::Block(blocking)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_agent::{
        NarrationVerifierResult, VerifierFinding, VerifierFindingKind, VerifierSeverity,
    };

    const ALL_KINDS: [VerifierFindingKind; 7] = [
        VerifierFindingKind::MissingCheck,
        VerifierFindingKind::MissingRollExecution,
        VerifierFindingKind::MissingEffect,
        VerifierFindingKind::InventedEffect,
        VerifierFindingKind::OmittedVisibleResult,
        VerifierFindingKind::ManualRollRequest,
        VerifierFindingKind::SecretLeak,
    ];

    const ALL_SEVERITIES: [VerifierSeverity; 2] =
        [VerifierSeverity::Blocker, VerifierSeverity::Warning];

    fn result_with(
        kind: VerifierFindingKind,
        severity: VerifierSeverity,
    ) -> NarrationVerifierResult {
        NarrationVerifierResult {
            accepted: severity != VerifierSeverity::Blocker,
            findings: vec![VerifierFinding {
                kind,
                severity,
                detail: "d".into(),
            }],
            next_required_action: None,
        }
    }

    /// 穷举 kind×severity (7×2=14)：仅 (Blocker × {SecretLeak,InventedEffect,ManualRollRequest})
    /// → Block；其余 11 组合 → Allow。
    #[test]
    fn exhaustive_kind_severity_matrix() {
        for kind in ALL_KINDS {
            for severity in ALL_SEVERITIES {
                let decision = presentation_gate_decision(&result_with(kind, severity));
                let should_block =
                    severity == VerifierSeverity::Blocker && BLOCKING_KINDS.contains(&kind);
                if should_block {
                    match decision {
                        PresentationGate::Block(fs) => {
                            assert_eq!(fs.len(), 1);
                            assert_eq!(fs[0].kind, kind);
                        }
                        _ => panic!("expected Block for ({kind:?}, {severity:?})"),
                    }
                } else {
                    assert_eq!(
                        decision,
                        PresentationGate::Allow,
                        "expected Allow for ({kind:?}, {severity:?})"
                    );
                }
            }
        }
    }

    /// 白名单恰好这三个 kind（锁定，widening 会令此红）。
    #[test]
    fn blocking_kinds_whitelist_is_locked() {
        assert_eq!(BLOCKING_KINDS.len(), 3);
        assert!(BLOCKING_KINDS.contains(&VerifierFindingKind::SecretLeak));
        assert!(BLOCKING_KINDS.contains(&VerifierFindingKind::InventedEffect));
        assert!(BLOCKING_KINDS.contains(&VerifierFindingKind::ManualRollRequest));
    }

    #[test]
    fn empty_findings_allow() {
        let r = NarrationVerifierResult {
            accepted: true,
            findings: vec![],
            next_required_action: None,
        };
        assert_eq!(presentation_gate_decision(&r), PresentationGate::Allow);
    }

    /// 多 finding 混合：只收集触发阻断的子集，warning/补账类被过滤。
    #[test]
    fn mixed_findings_collect_only_blocking_subset() {
        let r = NarrationVerifierResult {
            accepted: false,
            findings: vec![
                VerifierFinding {
                    kind: VerifierFindingKind::MissingCheck,
                    severity: VerifierSeverity::Blocker,
                    detail: "rd".into(),
                },
                VerifierFinding {
                    kind: VerifierFindingKind::SecretLeak,
                    severity: VerifierSeverity::Blocker,
                    detail: "leak".into(),
                },
                VerifierFinding {
                    kind: VerifierFindingKind::SecretLeak,
                    severity: VerifierSeverity::Warning,
                    detail: "warn-leak".into(),
                },
            ],
            next_required_action: None,
        };
        match presentation_gate_decision(&r) {
            PresentationGate::Block(fs) => {
                assert_eq!(fs.len(), 1, "only Blocker SecretLeak blocks");
                assert_eq!(fs[0].detail, "leak");
            }
            _ => panic!("expected Block"),
        }
    }
}
