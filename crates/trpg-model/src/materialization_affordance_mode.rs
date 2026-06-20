//! NPC/场景物化可供性开关（`TRPG_MATERIALIZATION_AFFORDANCE_MODE`）。
//!
//! 控制每回合 context 装配时是否从当前场景的 `referenced_npc_ids` 派生
//! `RuntimeState.active_npc_ids`（axis-1 NPC 激活，DP-2 "hybrid"）。
//!
//! | 值          | 行为                                             |
//! |-------------|--------------------------------------------------|
//! | `off`(默认) | 严格无操作；调用方提供的 active_npc_ids 原样保留 |
//! | `shadow`    | 派生并记录，但不替换已有非空集合                 |
//! | `enforce`   | 与 shadow 相同；命名区分"强制生效"语义           |
//!
//! # OFF 严格无操作保证（s17 架构修正）
//!
//! 当标志为 `Off` 时，调用方（`trpg-api`）传入的 `active_npc_ids`**不得**被
//! 修改——无论是非空还是空。这是基线字节一致性的前提。
//!
//! # 纯函数测试
//!
//! 所有测试使用 [`MaterializationAffordanceMode::parse`]，避免并发 `set_var`
//! 数据竞争（遵循 `lazy_object_schema_policy` 的同等约定）。

/// NPC 激活派生的运行时开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaterializationAffordanceMode {
    /// 默认：严格无操作，调用方 active_npc_ids 完全保留。
    #[default]
    Off,
    /// 仅在 active_npc_ids 为空时从场景派生；不覆盖非空调用方集合。
    Shadow,
    /// 语义同 Shadow（`enforce` 命名供未来区分全覆盖路径时使用）。
    Enforce,
}

impl MaterializationAffordanceMode {
    /// 从环境变量 `TRPG_MATERIALIZATION_AFFORDANCE_MODE` 读取。
    /// 未设置 / 无法识别 → `Off`（fail-closed）。
    pub fn from_env() -> Self {
        Self::parse(std::env::var("TRPG_MATERIALIZATION_AFFORDANCE_MODE").ok().as_deref())
    }

    /// 纯函数解析，测试友好（不读 env）。
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("shadow") => Self::Shadow,
            Some("enforce") => Self::Enforce,
            _ => Self::Off,
        }
    }

    /// 标志是否激活（Shadow 或 Enforce）。
    #[inline]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Shadow | Self::Enforce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off() {
        assert_eq!(MaterializationAffordanceMode::default(), MaterializationAffordanceMode::Off);
    }

    #[test]
    fn parse_off_variants() {
        // 未设置、空字符串、任意未知值 → Off
        for raw in [None, Some(""), Some("0"), Some("off"), Some("false"), Some("garbage")] {
            assert_eq!(
                MaterializationAffordanceMode::parse(raw),
                MaterializationAffordanceMode::Off,
                "expected Off for {:?}",
                raw
            );
        }
    }

    #[test]
    fn parse_shadow_variants() {
        for raw in ["shadow", "Shadow", "SHADOW", "  shadow  "] {
            assert_eq!(
                MaterializationAffordanceMode::parse(Some(raw)),
                MaterializationAffordanceMode::Shadow,
                "expected Shadow for {:?}",
                raw
            );
        }
    }

    #[test]
    fn parse_enforce_variants() {
        for raw in ["enforce", "Enforce", "ENFORCE"] {
            assert_eq!(
                MaterializationAffordanceMode::parse(Some(raw)),
                MaterializationAffordanceMode::Enforce,
                "expected Enforce for {:?}",
                raw
            );
        }
    }

    #[test]
    fn is_active_semantics() {
        assert!(!MaterializationAffordanceMode::Off.is_active());
        assert!(MaterializationAffordanceMode::Shadow.is_active());
        assert!(MaterializationAffordanceMode::Enforce.is_active());
    }
}
