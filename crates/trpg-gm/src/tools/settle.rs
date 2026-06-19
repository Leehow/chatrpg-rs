use crate::ledger::TurnLedger;
use crate::tools::{ToolCtx, ToolOutput};
use anyhow::Result;
use serde_json::{json, Value};
use trpg_model::*;

/// roll_check 与 request_player_roll（桌面骰权政策转系统）共用的系统结算尾段。
/// 前置条件：契约已构建并完成盖章（stamp_scene_intent）。
pub(crate) async fn settle_system_check(
    ctx: &ToolCtx<'_>,
    ledger: &mut TurnLedger,
    kernel: Option<&RuleKernel>,
    scene_intent: Option<&SceneMechanicIntent>,
    parameter_overridden: bool,
    contract: &CheckContract,
) -> Result<ToolOutput> {
    // 契约落 check_contracts 表（对齐旧路径 persist_agent_plan 的 db.insert_check_contract；
    // 只入内存 ledger 的话 Task 11 验收 SQL 对 agent 路径恒为空）。
    ctx.engine
        .db
        .insert_check_contract(contract, "created")
        .await?;
    ledger.record_contract(contract);
    let exec = ctx
        .engine
        .execute_system_roll_bundle(&ctx.request.session_id, &ctx.request.turn_id, contract)
        .await?;
    ledger.record_execution(&exec);
    // B3：成功度的剧情/机制语义随结果带回（success_bands[*].semantics 渲染）。
    // None → 整键缺省（fail-closed：裸 band 仍在 outcome 里，绝不编造语义）。
    let band_line = kernel.and_then(|k| band_semantics_line(&k.dice_core, &exec.primary.outcome));
    let mut out = json!({
        "check_id": contract.check_id,
        "check_label": contract.check_label,
        "roll_policy": exec.roll_policy,
        "outcome": exec.primary.outcome,
        "primary_roll": exec.primary.roll.result,
        "committed_patch_count": exec.primary.committed_patches.len(),
        "committed_patches": exec.primary.committed_patches,
        "followup_count": exec.followups.len()
    });
    let followups = summarize_followup_results(&exec.followups);
    if !followups.is_empty() {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("followups".into(), json!(followups));
        }
    }
    if let Some(line) = band_line {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("band_semantics".into(), json!(line));
        }
    }
    // C4：结算后 effect_policy Rust 强制执行（spec §6"效果不留给叙事"——结算完成后
    // 由 Rust 立即执行、不经叙事；outcome 无 success 布尔则 fail-closed 跳过并标记）。
    if let Some(intent) = scene_intent {
        if parameter_overridden {
            if let Some(obj) = out.as_object_mut() {
                obj.insert("parameter_overridden_from_intent".into(), json!(true));
            }
        }
        crate::scene_policy::run_policy_after_settlement(
            ctx.engine,
            ctx.request,
            intent,
            &exec.primary.outcome,
            &mut out,
            ledger,
        )
        .await?;
    }
    // B4：本回合结算新产的机械债务当场带回（spec §5.2 agent 当场看见）。
    // 轻查询：复用 list_open（回合内行数极小），按 turn_id 过滤本回合新产；
    // 查询失败按空处理（fail-closed，不阻断结果）；空数组时整键缺省。
    let dues: Vec<Value> = ctx.engine.db.list_open_mechanic_dues(&ctx.request.session_id).await
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.turn_id == ctx.request.turn_id)
        .map(|d| json!({"due_id": d.due_id, "threshold_desc": d.threshold_desc, "followup_procedure_id": d.followup_procedure_id, "evidence": d.evidence}))
        .collect();
    if !dues.is_empty() {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("dues".into(), json!(dues));
        }
    }
    Ok(ToolOutput::ok(out))
}

fn summarize_followup_results(results: &[CheckResultRecord]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            json!({
                "check_id": result.check_id,
                "roll_expression": result.roll.expression,
                "roll_visibility": result.roll.visibility,
                "roll_result": result.roll.result,
                "outcome": result.outcome,
                "committed_patch_count": result.committed_patches.len(),
                "committed_patches": result.committed_patches,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use trpg_model::{ActorKind, CheckResultRecord, DiceRollRecord, RollVisibility};

    fn result_with_patch() -> CheckResultRecord {
        CheckResultRecord {
            check_id: "check_effect_1".into(),
            roll: DiceRollRecord {
                roll_id: "roll_effect_1".into(),
                session_id: "session_test".into(),
                turn_id: "turn_test".into(),
                check_id: Some("check_effect_1".into()),
                roller_kind: ActorKind::PlayerCharacter,
                roller_id: Some("pc.current".into()),
                visibility: RollVisibility::PublicGmRoll,
                expression: "1d10+2".into(),
                result: json!({"mode":"rolled","expression":"1d10+2","rolls":[7],"modifier":2,"total":9}),
                seed_commitment: "seed".into(),
                revealed_at: Some(Utc::now()),
                created_at: Utc::now(),
            },
            outcome: json!({"success":true,"degree":"strong_success","total":9}),
            committed_patches: vec![StatePatch::ActorHpDelta {
                actor_id: "npc.opposition".into(),
                from: Some(10),
                to: Some(1),
                delta: -9,
                reason: "parameter_facet_executor_hp_delta".into(),
            }],
            created_at: Utc::now(),
        }
    }

    #[test]
    fn followup_summary_exposes_roll_and_committed_patches() {
        let followups = summarize_followup_results(&[result_with_patch()]);
        assert_eq!(followups[0]["check_id"], "check_effect_1");
        assert_eq!(followups[0]["roll_expression"], "1d10+2");
        assert_eq!(followups[0]["roll_result"]["total"], 9);
        assert_eq!(followups[0]["committed_patches"][0]["op"], "actor_hp_delta");
        assert_eq!(
            followups[0]["committed_patches"][0]["actor_id"],
            "npc.opposition"
        );
    }
}
