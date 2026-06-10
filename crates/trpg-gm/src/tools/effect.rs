use crate::ledger::TurnLedger;
use crate::tools::{GmTool, ToolCtx, ToolError, ToolOutput, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use trpg_mechanics::RefereeCombatService;
use trpg_model::{ParameterOperation, Visibility};

#[derive(Debug, Clone, Deserialize)]
pub struct ApplyEffectArgs {
    pub target_actor: String,
    pub parameter_path: Option<String>,
    pub track_id: Option<String>,
    pub op: String,
    pub amount: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangeTrackArgs {
    pub bucket: String,
    pub id: String,
    pub op: String,
    pub amount: Option<f64>,
    pub value: Option<String>,
    pub kind: Option<String>,
    pub category: Option<String>,
}

pub fn parse_apply_effect_args(value: Value) -> Result<ApplyEffectArgs> {
    let args: ApplyEffectArgs = serde_json::from_value(value).map_err(|e| ToolError::recoverable("invalid_arguments", format!("apply_effect arguments invalid: {e}"), None))?;
    let path_count = args.parameter_path.iter().filter(|s| !s.trim().is_empty()).count() + args.track_id.iter().filter(|s| !s.trim().is_empty()).count();
    if path_count != 1 {
        return Err(ToolError::recoverable("invalid_arguments", "provide exactly one of parameter_path or track_id", Some("Use parameter_path for actor fields or track_id for resources/tracks.".to_string())));
    }
    Ok(args)
}

pub fn parse_change_track_args(value: Value) -> Result<ChangeTrackArgs> {
    serde_json::from_value(value).map_err(|e| ToolError::recoverable("invalid_arguments", format!("change_track arguments invalid: {e}"), None))
}

/// PURE: synthesize the effect parameter_path. `track_id` maps to
/// `resources.{id}.current` — the only prefixed form that
/// `trpg_model::resolve_resource_track_id` resolves against kernel
/// resource_tracks (a `tracks.` prefix would make "tracks" the candidate id
/// and never match, sending mechanics into blocked_resource_delta).
pub fn effect_parameter_path(args: &ApplyEffectArgs) -> String {
    match args.parameter_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(path) => path.to_string(),
        None => format!("resources.{}.current", args.track_id.as_deref().unwrap_or_default().trim()),
    }
}

fn op_from_str(s: &str) -> Result<ParameterOperation> {
    match s.trim().to_ascii_lowercase().as_str() {
        "add" => Ok(ParameterOperation::Add),
        "subtract" => Ok(ParameterOperation::Subtract),
        "set" => Ok(ParameterOperation::Set),
        "add_condition" => Ok(ParameterOperation::AddCondition),
        "remove_condition" => Ok(ParameterOperation::RemoveCondition),
        "advance_clock" => Ok(ParameterOperation::AdvanceClock),
        "mark_revealed" => Ok(ParameterOperation::MarkRevealed),
        other => Err(ToolError::recoverable("invalid_arguments", format!("invalid operation: {other}"), None)),
    }
}

pub struct ApplyEffectTool;

#[async_trait]
impl GmTool for ApplyEffectTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "apply_effect", schema: json!({"type":"function","function":{"name":"apply_effect","description":"Apply a direct state/resource effect through mechanics.","parameters":{"type":"object","properties":{"target_actor":{"type":"string"},"parameter_path":{"type":"string"},"track_id":{"type":"string"},"op":{"type":"string","enum":["add","subtract","set","add_condition","remove_condition","advance_clock","mark_revealed"]},"amount":{"type":"integer"},"reason":{"type":"string"}},"required":["target_actor","op","amount","reason"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_apply_effect_args(args)?;
        let parameter_path = effect_parameter_path(&args);
        let service = RefereeCombatService::new(ctx.engine.db.clone());
        let outcome = service.apply_direct_effect(&ctx.request.session_id, &ctx.request.ruleset_id, ctx.request.module_id.as_deref(), "pc.current", &args.target_actor, &parameter_path, op_from_str(&args.op)?, args.amount, &args.reason, Visibility::GmOnly).await?;
        ledger.record_effect(&outcome.effect);
        for impact in &outcome.impacts { ledger.record_impact(impact); }
        Ok(ToolOutput::ok(json!({"effect_id": outcome.effect.effect_id, "impact_count": outcome.impacts.len(), "patch_count": outcome.patches.len()})))
    }
}

pub struct ChangeTrackTool;

#[async_trait]
impl GmTool for ChangeTrackTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: "change_track", schema: json!({"type":"function","function":{"name":"change_track","description":"Apply growth or explicit track movement.","parameters":{"type":"object","properties":{"bucket":{"type":"string","enum":["tracks","stats","skills","field"]},"id":{"type":"string"},"op":{"type":"string","enum":["set","add"]},"amount":{"type":"number"},"value":{"type":"string"},"kind":{"type":"string"},"category":{"type":"string"}},"required":["bucket","id","op"]}}}) }
    }

    async fn call(&self, ctx: &ToolCtx<'_>, _ledger: &mut TurnLedger, args: Value) -> Result<ToolOutput> {
        let args = parse_change_track_args(args)?;
        let text = args.value.as_deref();
        let amount = args.amount.unwrap_or(0.0);
        let ok = ctx.engine.apply_track_change(&ctx.request.session_id, "pc.current", &args.bucket, &args.id, &args.op, amount, text, args.kind.as_deref(), args.category.as_deref()).await?;
        Ok(ToolOutput::ok(json!({"changed": ok, "bucket": args.bucket, "id": args.id})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_effect_requires_one_target_path() {
        let err = parse_apply_effect_args(json!({"target_actor":"npc.opposition","op":"subtract","amount":3,"reason":"hit"})).unwrap_err();
        assert!(err.to_string().contains("invalid_arguments"));
    }

    #[test]
    fn synthesized_track_path_resolves_against_sample_kernel() {
        use trpg_model::{resolve_resource_track_id, RuleKernel};
        let kernel = RuleKernel {
            resource_tracks: vec![json!({"id": "sanity", "name": "Sanity"}), json!({"id": "hit_points", "kind": "hp"})],
            ..Default::default()
        };
        let args = parse_apply_effect_args(json!({"target_actor":"npc.opposition","track_id":"sanity","op":"subtract","amount":3,"reason":"shock"})).unwrap();
        let path = effect_parameter_path(&args);
        assert_eq!(path, "resources.sanity.current");
        assert_eq!(resolve_resource_track_id(&path, &kernel).as_deref(), Some("sanity"));
    }

    #[test]
    fn explicit_parameter_path_passes_through_unchanged() {
        let args = parse_apply_effect_args(json!({"target_actor":"pc.current","parameter_path":"resources.hit_points.current","op":"subtract","amount":2,"reason":"hit"})).unwrap();
        assert_eq!(effect_parameter_path(&args), "resources.hit_points.current");
    }

    #[test]
    fn change_track_accepts_string_value_without_amount() {
        let args = parse_change_track_args(json!({"bucket":"field","id":"class","op":"set","value":"solo"})).unwrap();
        assert_eq!(args.value.as_deref(), Some("solo"));
        assert_eq!(args.amount.unwrap_or(0.0), 0.0);
    }
}
