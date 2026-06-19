use crate::FacetDecision;
use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use trpg_model::{
    ActorKind, ActorRef, CheckContract, CheckModifier, CheckResultRecord, CheckStakes,
    CheckTargetModel, DiceRollRecord, EffectContract, EffectKind, EffectTargetKind,
    FacetExecutionStatus, ParameterImpact, ParameterOperation, RollAuthority, RollDisclosurePolicy,
    RollVisibility, RulingConfidence, RulingStatus, StatePatch, TestedParameter, Visibility,
};
use uuid::Uuid;

/// 无检定直接效果的公共产物。
#[derive(Debug, Clone)]
pub struct DirectEffectOutcome {
    pub effect: EffectContract,
    pub impacts: Vec<ParameterImpact>,
    pub patches: Vec<StatePatch>,
}

/// PURE：operation → EffectKind 语义映射（EffectKind 没有 ResourceDelta 变体，
/// 真实变体见 trpg-model：Damage/Healing/ResourceSpend/NarrativeConsequence…）。
pub fn effect_kind_for_operation(operation: ParameterOperation) -> EffectKind {
    match operation {
        ParameterOperation::Subtract => EffectKind::Damage,
        ParameterOperation::Add => EffectKind::Healing,
        _ => EffectKind::NarrativeConsequence,
    }
}

/// PURE：合成契约 + 合成结果（roll.result.total = amount，无 RNG），不触 DB。
/// CheckContract 没有 metadata 字段（也不存在 metadata_mut）——operation/source
/// 信息只进最终 EffectContract.metadata。
#[allow(clippy::too_many_arguments)]
pub fn build_direct_effect_contract(
    session_id: &str,
    ruleset_id: &str,
    module_id: Option<&str>,
    source_actor_id: &str,
    target_actor_id: &str,
    parameter_path: &str,
    amount: i64,
    reason: &str,
) -> (CheckContract, CheckResultRecord) {
    let check_id = format!("check_direct_effect_{}", Uuid::new_v4().simple());
    let turn_id = "direct_effect".to_string();
    let contract = CheckContract {
        check_id: check_id.clone(),
        session_id: session_id.to_string(),
        turn_id: turn_id.clone(),
        ruleset_id: ruleset_id.to_string(),
        module_id: module_id.map(str::to_string),
        initiator: ActorRef {
            actor_id: source_actor_id.to_string(),
            actor_kind: ActorKind::PlayerCharacter,
            display_name: Some("effect source".to_string()),
        },
        target_actor: Some(ActorRef {
            actor_id: target_actor_id.to_string(),
            actor_kind: ActorKind::Npc,
            display_name: Some("effect target".to_string()),
        }),
        opposition: trpg_model::OppositionModel::NoMechanicalOpposition,
        action_summary: reason.to_string(),
        intent_kind: "effect_roll".to_string(),
        check_label: "direct effect".to_string(),
        dice_expression: amount.to_string(),
        modifiers: Vec::<CheckModifier>::new(),
        target: CheckTargetModel::UnknownUntilLookup,
        tested_parameter: Some(TestedParameter {
            domain: None,
            key: parameter_path.to_string(),
            label: parameter_path.to_string(),
        }),
        opponent_tested_parameter: None,
        actor_snapshot_ids: vec![target_actor_id.to_string()],
        source_refs: vec![],
        learned_packet_ids: vec![],
        roll_visibility: RollVisibility::PrivateGmRoll,
        roll_authority: RollAuthority::System,
        disclosure: RollDisclosurePolicy::for_visibility(RollVisibility::PrivateGmRoll),
        stakes: CheckStakes {
            before_roll_public: reason.to_string(),
            success_public: "The direct effect is applied.".to_string(),
            failure_public: "The direct effect could not be applied.".to_string(),
            critical_public: None,
            fumble_public: None,
            success_patches_allowed: vec![],
            failure_patches_allowed: vec![],
            irreversible: false,
        },
        confidence: RulingConfidence::Medium,
        ruling_status: RulingStatus::Provisional,
        advice_refs: vec!["gm_agent.apply_effect".to_string()],
        expires_at_turn: None,
    };
    let roll = DiceRollRecord {
        roll_id: format!("roll_{}", Uuid::new_v4().simple()),
        session_id: session_id.to_string(),
        turn_id,
        check_id: Some(check_id.clone()),
        roller_kind: ActorKind::System,
        roller_id: Some("gm_agent".to_string()),
        visibility: RollVisibility::PrivateGmRoll,
        expression: amount.to_string(),
        result: json!({"total": amount}),
        seed_commitment: "direct_effect_no_rng".to_string(),
        revealed_at: None,
        created_at: Utc::now(),
    };
    let result = CheckResultRecord {
        check_id,
        roll,
        outcome: json!({"success": true, "total": amount}),
        committed_patches: vec![],
        created_at: Utc::now(),
    };
    (contract, result)
}

impl crate::RefereeCombatService {
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_direct_effect(
        &self,
        session_id: &str,
        ruleset_id: &str,
        module_id: Option<&str>,
        source_actor_id: &str,
        target_actor_id: &str,
        parameter_path: &str,
        operation: ParameterOperation,
        amount: i64,
        reason: &str,
        visibility: Visibility,
    ) -> Result<DirectEffectOutcome> {
        let (contract, mut result) = build_direct_effect_contract(
            session_id,
            ruleset_id,
            module_id,
            source_actor_id,
            target_actor_id,
            parameter_path,
            amount,
            reason,
        );
        // 显式落点：直接效果的 path/op 由 GM 裁量给定，必须绕过
        // resolve_facet_decision 的 hp.current fallback（无绑定 facet 时它恒回落
        // hp.current，parameter_path/operation 会形同虚设）。FacetDecision 与
        // apply_effect_roll_with_decision 均为 crate 根私有项，本文件是同 crate
        // 子模块可直接访问。
        let decision = FacetDecision {
            target_kind: EffectTargetKind::Actor,
            target_id: target_actor_id.to_string(),
            parameter_path: parameter_path.to_string(),
            operation,
            source_facet_binding_ids: vec![],
            source_refs: vec![],
            status: FacetExecutionStatus::AppliedProvisional,
            provisional_reason: Some(format!("GM agent direct effect: {reason}")),
        };
        let applied = self
            .apply_effect_roll_with_decision(&contract, &result, Some(decision))
            .await?;
        result.committed_patches.extend(applied.patches.clone());
        // B4 watcher：直接效果落账后阈值检测（与 apply_outcome_resource_tracks
        // 同一纯函数单点）。fail-closed：kernel 缺失/impact before-after 取整不成/
        // parameter_path 解析不到 kernel 轨 → 跳过；watcher 故障绝不中断主链。
        if let Ok(Some(kernel)) = self.db.load_rule_kernel(ruleset_id).await {
            let mut crossings: Vec<crate::watcher::ThresholdCrossing> = Vec::new();
            for impact in &applied.effect.impacts {
                let (Some(b), Some(a)) = (
                    impact
                        .before
                        .as_ref()
                        .and_then(|v| v.as_i64())
                        .and_then(|n| i32::try_from(n).ok()),
                    impact
                        .after
                        .as_ref()
                        .and_then(|v| v.as_i64())
                        .and_then(|n| i32::try_from(n).ok()),
                ) else {
                    continue;
                };
                let Some(track_id) =
                    trpg_model::resolve_resource_track_id(&impact.parameter_path, &kernel)
                else {
                    continue;
                };
                let Some(track) = kernel.resource_tracks.iter().find(|t| {
                    t.get("id").and_then(|v| v.as_str()).map(str::trim) == Some(track_id.as_str())
                }) else {
                    continue;
                };
                let owner_kind = track
                    .get("owner_kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("actor");
                crossings.extend(crate::watcher::detect_crossings(
                    track,
                    owner_kind,
                    target_actor_id,
                    b,
                    a,
                    impact.operation,
                ));
            }
            if !crossings.is_empty() {
                let _ = self
                    .detect_threshold_dues(&contract, &result, &kernel, &crossings)
                    .await;
            }
        }
        let effect = EffectContract {
            effect_id: applied.effect.effect_resolution_id.clone(),
            source_event_id: None,
            effect_kind: effect_kind_for_operation(operation),
            target_actor_ids: vec![target_actor_id.to_string()],
            source_refs: applied.effect.source_refs.clone(),
            learned_packet_ids: vec![],
            deterministic_parts: vec![],
            pending_rolls: vec![],
            proposed_patches: applied.patches.clone(),
            visibility,
            confidence: RulingConfidence::Medium,
            metadata: json!({"reason": reason, "source": "direct_effect", "source_actor_id": source_actor_id, "parameter_path": parameter_path, "operation": operation.as_str()}),
        };
        Ok(DirectEffectOutcome {
            effect,
            impacts: applied.effect.impacts,
            patches: applied.patches,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_kind_maps_by_operation() {
        assert!(matches!(
            effect_kind_for_operation(ParameterOperation::Subtract),
            EffectKind::Damage
        ));
        assert!(matches!(
            effect_kind_for_operation(ParameterOperation::Add),
            EffectKind::Healing
        ));
        assert!(matches!(
            effect_kind_for_operation(ParameterOperation::Set),
            EffectKind::NarrativeConsequence
        ));
    }

    #[test]
    fn synthetic_contract_carries_amount_and_target() {
        let (contract, result) = build_direct_effect_contract(
            "s",
            "rs",
            None,
            "pc.current",
            "npc.opposition",
            "sanity.current",
            4,
            "shock",
        );
        assert_eq!(
            contract.target_actor.as_ref().unwrap().actor_id,
            "npc.opposition"
        );
        assert_eq!(
            contract.tested_parameter.as_ref().unwrap().key,
            "sanity.current"
        );
        assert_eq!(contract.roll_visibility, RollVisibility::PrivateGmRoll);
        assert_eq!(contract.roll_authority, RollAuthority::System);
        assert_eq!(
            result.roll.result.get("total").and_then(|v| v.as_i64()),
            Some(4)
        );
        assert_eq!(result.check_id, contract.check_id);
    }
}
