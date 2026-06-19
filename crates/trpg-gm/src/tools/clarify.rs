//! P2 步骤4：无状态 `ask_clarification` 工具（设计4 §9.5 "最多保留 ask_clarification"）。
//!
//! 唯一只读、零副作用工具：不碰 DB / ledger / obligation / frame，只把 question 透传回
//! `{"clarification": question}`。**不**进 ToolRegistry::standard()（避免污染 Adjudicator 现
//! 15 工具 schema 字节稳定性）——只由 `ToolRegistry::narrator_safe()` 在过滤掉全部 Mutating
//! 工具后尾部追加。capability()==ReadOnly。

use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCapability, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct AskClarificationTool;

#[async_trait]
impl GmTool for AskClarificationTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_clarification",
            schema: json!({"type":"function","function":{"name":"ask_clarification","description":"Ask the player a clarifying question instead of narrating. Read-only: performs no mechanical or world-state mutation.","parameters":{"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}}}),
        }
    }

    fn capability(&self) -> ToolCapability {
        ToolCapability::ReadOnly
    }

    /// 纯透传：无 DB / ledger / IO。question 缺失/非字符串 → recoverable invalid_arguments。
    async fn call(
        &self,
        _ctx: &ToolCtx<'_>,
        _ledger: &mut TurnLedger,
        args: Value,
    ) -> Result<ToolOutput> {
        let question = args
            .get("question")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ToolError::recoverable(
                    "invalid_arguments",
                    "ask_clarification requires a string `question`",
                    Some("Emit {\"question\":\"...\"} matching the tool schema.".to_string()),
                )
            })?;
        Ok(ToolOutput::ok(json!({ "clarification": question })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::TurnLedger;
    use crate::tools::ToolCapability;

    #[test]
    fn ask_clarification_is_read_only() {
        assert_eq!(
            AskClarificationTool.capability(),
            ToolCapability::ReadOnly,
            "ask_clarification must be ReadOnly (no side effects)"
        );
    }

    #[test]
    fn spec_name_is_stable() {
        assert_eq!(AskClarificationTool.spec().name, "ask_clarification");
    }

    /// call() 结构正确（透传 question）且无副作用：ledger 调用前后快照字节相等
    /// （工具从不写 ledger）。无 DB/engine：dummy ctx 即可，因 call 不碰它们。
    #[tokio::test]
    async fn call_passes_through_question_with_no_side_effects() {
        let mut ledger = TurnLedger::new();
        let before = serde_json::to_vec(ledger.snapshot()).unwrap();
        let ctx = crate::tools::ToolCtx {
            engine: &dummy_engine(),
            request: &dummy_request(),
            state: &trpg_model::RuntimeState::default(),
            scene_extractor: None,
            obligations: None,
            data_dir: None,
            current_mode: None,
            opposed_binding: None,
            nominated_reveals: None,
        };
        let out = AskClarificationTool
            .call(&ctx, &mut ledger, json!({"question": "去哪个门?"}))
            .await
            .expect("ok");
        assert_eq!(
            out.result.get("clarification").and_then(Value::as_str),
            Some("去哪个门?")
        );
        assert!(out.awaiting_player_roll.is_none());
        let after = serde_json::to_vec(ledger.snapshot()).unwrap();
        assert_eq!(before, after, "ask_clarification must not mutate ledger");
    }

    #[tokio::test]
    async fn missing_question_is_recoverable_error() {
        let mut ledger = TurnLedger::new();
        let ctx = crate::tools::ToolCtx {
            engine: &dummy_engine(),
            request: &dummy_request(),
            state: &trpg_model::RuntimeState::default(),
            scene_extractor: None,
            obligations: None,
            data_dir: None,
            current_mode: None,
            opposed_binding: None,
            nominated_reveals: None,
        };
        let err = match AskClarificationTool
            .call(&ctx, &mut ledger, json!({}))
            .await
        {
            Ok(_) => panic!("missing question must error"),
            Err(e) => e,
        };
        let tool_err = err.downcast_ref::<ToolError>().expect("ToolError");
        assert_eq!(tool_err.code, "invalid_arguments");
        assert!(tool_err.recoverable);
    }

    fn dummy_engine() -> trpg_runtime::RuntimeEngine {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://chatrpg:chatrpg@localhost:54347/chatrpg")
            .expect("lazy pool");
        trpg_runtime::RuntimeEngine::new(trpg_db::Db { pool })
    }

    fn dummy_request() -> trpg_model::ContextRequest {
        trpg_model::ContextRequest {
            ruleset_id: "rs".to_string(),
            module_id: None,
            session_id: "s".to_string(),
            turn_id: "t".to_string(),
            viewer: trpg_model::VisibilityProfile::gm(),
            token_budget: trpg_model::TokenBudget::default(),
        }
    }
}
