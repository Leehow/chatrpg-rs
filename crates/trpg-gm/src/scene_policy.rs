//! C4：roll_check(scene_mechanic_id) 的 effect_policy → 既有原语映射执行
//! （spec §6"效果不留给叙事"）。结算完成后由 Rust 立即执行、不经叙事。

use crate::ledger::TurnLedger;
use crate::tools::ToolError;
use serde_json::{json, Value};
use trpg_mechanics::RefereeCombatService;
use trpg_model::{
    CheckContract, CheckTargetModel, ContextRequest, EffectPatchIntent, EffectPolicy,
    ModuleGraph, SceneExtractionStatus, SceneMechanicIntent, StatePatch, TimeAmount, TimeScale,
    Visibility, WorldEventKind,
};
use trpg_object::ObjectService;
use trpg_runtime::RuntimeEngine;
use trpg_time::WorldTimeService;
use uuid::Uuid;

/// PURE：按 outcome 选分支。
pub(crate) fn select_intents(policy: &EffectPolicy, success: bool) -> &[EffectPatchIntent] {
    if success { &policy.on_success } else { &policy.on_failure }
}

/// PURE：difficulty Value → CheckTargetModel。认得的形态：{kind:"dv"|"static"|"target_number",
/// value:<num>} → StaticNumber{value, label=原 JSON 紧凑串}。其余（CoC 难度档字符串等）→ None
/// （保持 UnknownUntilLookup，交给 execute_system_roll_bundle 的 kernel defaults——fail-closed，
/// 形态映射非规则集分支，零硬编码）。
pub(crate) fn difficulty_to_target(difficulty: Option<&serde_json::Value>) -> Option<CheckTargetModel> {
    let v = difficulty?;
    let obj = v.as_object()?;
    let kind = obj.get("kind")?.as_str()?;
    if !matches!(kind, "dv" | "static" | "target_number") {
        return None;
    }
    let value = i32::try_from(obj.get("value")?.as_i64()?).ok()?;
    Some(CheckTargetModel::StaticNumber { value, label: v.to_string() })
}

/// PURE：当前场景查 intent。场景解析次序对标 module_scene_blocks_for_turn（runtime
/// L1513-1526）：scene_id 显式命中 → 该场景；否则回退首个 DeepExtracted 场景。
/// 在解析出的场景的 scene_mechanics 内按 intent_id 精确匹配（id 相等比较=合法字面匹配，
/// 护栏 §3.5.4）。
pub(crate) fn find_scene_intent<'a>(
    graph: &'a ModuleGraph,
    scene_id: Option<&str>,
    intent_id: &str,
) -> Option<&'a SceneMechanicIntent> {
    let node = scene_id
        .and_then(|sid| graph.scenes.iter().find(|s| s.node_id == sid))
        .or_else(|| {
            graph
                .scenes
                .iter()
                .find(|s| s.extraction_status == SceneExtractionStatus::DeepExtracted)
        })?;
    // 不跨场景兜底：当前场景没有就是没有，防把别处的 effect_policy 错绑。
    node.scene_mechanics.iter().find(|i| i.intent_id == intent_id)
}

/// PURE：不可执行 intent → CreateFact 记 "unexecutable_intent" 事实（可观测不静默）。
/// Other(v) 序列化即原 JSON（untagged），其余变体序列化为带 kind 的对象。
pub(crate) fn fold_unexecutable(intent: &EffectPatchIntent, reason: &str) -> StatePatch {
    let raw = serde_json::to_value(intent).unwrap_or(Value::Null);
    StatePatch::CreateFact {
        target: "scene.policy".to_string(),
        fact: json!({ "unexecutable_intent": raw }),
        reason: reason.to_string(),
    }
}

/// PURE：已执行 patch → 工具结果 JSON 摘要数组（op/target/数额；unexecutable 条目原样可见）。
pub(crate) fn patches_summary(patches: &[StatePatch]) -> Value {
    Value::Array(
        patches
            .iter()
            .map(|p| match p {
                StatePatch::SetTrack { target, value, .. } => json!({"op":"set_track","target":target,"amount":value}),
                StatePatch::ModifyTrack { target, amount, .. } => json!({"op":"modify_track","target":target,"amount":amount}),
                StatePatch::ObjectPatch { object_id, patch_json, .. } => json!({"op":"set_object_state","target":object_id,"patch":patch_json}),
                StatePatch::CreateFact { target, fact, .. } => json!({"op":"create_fact","target":target,"fact":fact}),
                other => serde_json::to_value(other).unwrap_or(Value::Null),
            })
            .collect(),
    )
}

/// 结算后强制执行 effect_policy（spec §6"效果不留给叙事"）。按 outcome 选
/// on_success/on_failure，逐条映射到既有原语；每条产出入账 ledger；整批执行完
/// 落一条 world_event(kind=EffectApplied, event_json.source="scene.policy")——
/// e2e SQL 可查的单一证据点。单条失败：记 unexecutable 事实 + 继续下一条
/// （不中断、不静默）。返回已执行的 StatePatch 列表（工具结果 JSON 摘要用）。
pub async fn apply_effect_policy(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    intent: &SceneMechanicIntent,
    success: bool,
    ledger: &mut TurnLedger,
) -> anyhow::Result<Vec<StatePatch>> {
    let reason = format!(
        "scene.policy:{} ({})",
        intent.intent_id,
        if success { "success" } else { "failure" }
    );
    let mut patches = Vec::new();
    for item in select_intents(&intent.effect_policy, success) {
        match execute_intent(engine, request, intent, item, &reason, ledger).await {
            Ok(patch) => patches.push(patch),
            Err(err) => {
                // 单条失败：记 unexecutable 事实 + 继续下一条（不中断、不静默）。
                let mut patch = fold_unexecutable(item, &reason);
                if let StatePatch::CreateFact { fact, .. } = &mut patch {
                    if let Some(obj) = fact.as_object_mut() {
                        obj.insert("error".into(), json!(err.to_string()));
                    }
                }
                patches.push(patch);
            }
        }
    }
    // world_event 证据单点（C6 验收 9 的 SQL 锚点，比散查三张表稳）。
    engine
        .record_world_event(
            &request.session_id,
            Some(&request.turn_id),
            None,
            WorldEventKind::EffectApplied,
            json!({
                "source": "scene.policy",
                "intent_id": intent.intent_id,
                "success": success,
                "patches": serde_json::to_value(&patches)?,
            }),
            Visibility::GmOnly,
        )
        .await?;
    Ok(patches)
}

/// 单条 intent → 既有原语。每分支 ≤15 行（映射表见计划 C4）。
async fn execute_intent(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    intent: &SceneMechanicIntent,
    item: &EffectPatchIntent,
    reason: &str,
    ledger: &mut TurnLedger,
) -> anyhow::Result<StatePatch> {
    match item {
        EffectPatchIntent::ModifyTrack { owner_id, track_id, op, amount, .. } => {
            let target_actor = owner_id.as_deref().unwrap_or("pc.current");
            // resources.{id}.current 是 resolve_resource_track_id 可解析的唯一前缀形
            // （对标 effect.rs effect_parameter_path）。
            let path = format!("resources.{track_id}.current");
            let operation = crate::tools::effect::op_from_str(op)?;
            let outcome = RefereeCombatService::new(engine.db.clone())
                .apply_direct_effect(&request.session_id, &request.ruleset_id, request.module_id.as_deref(), "scene.policy", target_actor, &path, operation, *amount, reason, Visibility::GmOnly)
                .await?;
            ledger.record_effect(&outcome.effect);
            for impact in &outcome.impacts {
                ledger.record_impact(impact);
            }
            Ok(track_patch(target_actor, track_id, op, *amount, reason))
        }
        EffectPatchIntent::SetObjectState { object_id, patch } => {
            let world_tick = WorldTimeService::new(engine.db.clone())
                .ensure_session_time(&request.session_id, None)
                .await?
                .world_tick;
            ObjectService::new(engine.db.clone())
                .apply_external_mechanical_patch(&request.session_id, object_id, patch.clone(), reason, world_tick)
                .await?;
            Ok(StatePatch::ObjectPatch {
                patch_id: format!("scene_policy_{}", Uuid::new_v4().simple()),
                object_id: Some(object_id.clone()),
                patch_json: patch.clone(),
                reason: reason.to_string(),
            })
        }
        EffectPatchIntent::CreateFact { target, fact } => Ok(StatePatch::CreateFact {
            target: target.clone(),
            fact: fact.clone(),
            reason: reason.to_string(),
        }),
        EffectPatchIntent::StartCountdown { label, amount, scale, payload } => {
            let time_scale = crate::tools::world::parse_time_scale(scale)?;
            // scale→TimeAmount 构造映射对标 AdvanceTimeTool（world.rs L71-75）。
            let time_amount = match time_scale {
                TimeScale::CombatRound => TimeAmount::combat_rounds(*amount),
                TimeScale::SceneBeat => TimeAmount::scene_beats(*amount),
                _ => TimeAmount::minutes(*amount),
            };
            let event = WorldTimeService::new(engine.db.clone())
                .schedule_in(&request.session_id, time_amount, WorldEventKind::ClockTick, json!({"label": label, "intent_id": intent.intent_id, "payload": payload}), Visibility::GmOnly, None)
                .await?;
            Ok(StatePatch::CreateFact {
                target: "scene.policy".to_string(),
                fact: json!({"countdown_scheduled": {"scheduled_event_id": event.scheduled_event_id, "label": label, "amount": amount, "scale": scale}}),
                reason: reason.to_string(),
            })
        }
        // fail-closed：不执行、不中断，折 unexecutable 事实（可观测）。
        EffectPatchIntent::Other(_) => Ok(fold_unexecutable(item, reason)),
    }
}

/// PURE：契约盖章（roll_check / request_player_roll 共用惯例）：difficulty 认得的
/// 形态 → StaticNumber target（认不出保持原 target，fail-closed）；结构化引用
/// `scene_mechanic:<intent_id>` 进 advice_refs——gate 结算路径据此恢复绑定执行
/// effect_policy。None → no-op。
pub(crate) fn stamp_scene_intent(contract: &mut CheckContract, intent: Option<&SceneMechanicIntent>) {
    let Some(intent) = intent else { return };
    if let Some(t) = difficulty_to_target(intent.difficulty.as_ref()) {
        contract.target = t;
    }
    contract.advice_refs.push(format!("scene_mechanic:{}", intent.intent_id));
}

/// roll_check 接线辅助：scene_mechanic_id → 当前场景 intent（按 intent_id 精确匹配，
/// 护栏 §3.5.4；load_module_graph 是"当前 module bundle 单一事实源"——P5 续抽后的
/// intents 可见）。None 入参 → Ok(None)（向后兼容，零额外 db 查询）。
pub(crate) async fn resolve_scene_intent(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    current_scene_id: Option<&str>,
    scene_mechanic_id: Option<&str>,
) -> anyhow::Result<Option<SceneMechanicIntent>> {
    let Some(id) = scene_mechanic_id else { return Ok(None) };
    let mid = request.module_id.as_deref().ok_or_else(|| {
        ToolError::recoverable("no_module_loaded", "scene_mechanic_id requires a module", None)
    })?;
    let graph = engine.db.load_module_graph(mid).await?.ok_or_else(|| {
        ToolError::recoverable("no_module_loaded", format!("module graph not loaded: {mid}"), None)
    })?;
    Ok(Some(
        find_scene_intent(&graph, current_scene_id, id)
            .ok_or_else(|| {
                ToolError::recoverable(
                    "scene_mechanic_not_found",
                    format!("scene mechanic not found in current scene: {id}"),
                    Some("Check the BP2 scene-mechanics block, or roll without scene_mechanic_id.".to_string()),
                )
            })?
            .clone(),
    ))
}

/// roll_check 接线辅助：结算后执行 policy 并把摘要写进结果 JSON。
/// outcome 无 success 布尔 → fail-closed 不执行，标 effect_policy_skipped（可观测不静默）。
pub(crate) async fn run_policy_after_settlement(
    engine: &RuntimeEngine,
    request: &ContextRequest,
    intent: &SceneMechanicIntent,
    outcome: &Value,
    out: &mut Value,
    ledger: &mut TurnLedger,
) -> anyhow::Result<()> {
    match outcome.get("success").and_then(Value::as_bool) {
        Some(success) => {
            let patches = apply_effect_policy(engine, request, intent, success, ledger).await?;
            if let Some(obj) = out.as_object_mut() {
                obj.insert("executed_patches".into(), patches_summary(&patches));
            }
        }
        None => {
            if let Some(obj) = out.as_object_mut() {
                obj.insert("effect_policy_skipped".into(), json!("outcome has no success field"));
            }
        }
    }
    Ok(())
}

/// PURE：ModifyTrack 执行结果折 StatePatch（set→SetTrack，加减→ModifyTrack 带符号）。
fn track_patch(target_actor: &str, track_id: &str, op: &str, amount: i64, reason: &str) -> StatePatch {
    let target = format!("{target_actor}.{track_id}");
    let op_norm = op.trim().to_ascii_lowercase();
    if op_norm == "set" {
        StatePatch::SetTrack { target, value: amount as i32, reason: reason.to_string() }
    } else {
        let signed = if op_norm == "subtract" { -(amount as i32) } else { amount as i32 };
        StatePatch::ModifyTrack { target, amount: signed, reason: reason.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trpg_model::SceneExtractionStatus;

    fn mt(track: &str) -> EffectPatchIntent {
        EffectPatchIntent::ModifyTrack {
            owner_kind: "actor".to_string(),
            owner_id: None,
            track_id: track.to_string(),
            op: "subtract".to_string(),
            amount: 1,
        }
    }

    #[test]
    fn select_intents_picks_branch() {
        let policy = EffectPolicy { on_success: vec![mt("a"), mt("b")], on_failure: vec![mt("c")] };
        assert_eq!(select_intents(&policy, true).len(), 2);
        assert_eq!(select_intents(&policy, false).len(), 1);
    }

    #[test]
    fn difficulty_to_target_maps_dv_and_static() {
        let t = difficulty_to_target(Some(&json!({"kind":"dv","value":13})));
        assert!(matches!(t, Some(CheckTargetModel::StaticNumber { value: 13, .. })), "dv 13 must map: {t:?}");
        let t = difficulty_to_target(Some(&json!({"kind":"target_number","value":50})));
        assert!(matches!(t, Some(CheckTargetModel::StaticNumber { value: 50, .. })), "target_number 50 must map: {t:?}");
        assert!(difficulty_to_target(Some(&json!("hard"))).is_none(), "bare string difficulty must stay None");
        assert!(difficulty_to_target(Some(&json!({"kind":"dv","value":"thirteen"}))).is_none(), "non-numeric value must stay None");
    }

    fn scene(node_id: &str, intent_id: &str) -> trpg_model::ScenarioNode {
        trpg_model::ScenarioNode {
            node_id: node_id.to_string(),
            extraction_status: SceneExtractionStatus::DeepExtracted,
            scene_mechanics: vec![SceneMechanicIntent {
                intent_id: intent_id.to_string(),
                description: "a triggering action".to_string(),
                tested_parameter: "brawling".to_string(),
                difficulty: None,
                effect_policy: EffectPolicy::default(),
                source_anchor: "# Page 1".to_string(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn find_scene_intent_resolves_current_scene_then_falls_back() {
        let graph = ModuleGraph {
            scenes: vec![scene("sc1", "intent_a"), scene("sc2", "intent_b")],
            ..Default::default()
        };
        // scene_id 显式命中 → 该场景内查 intent_b。
        assert_eq!(
            find_scene_intent(&graph, Some("sc2"), "intent_b").map(|i| i.intent_id.as_str()),
            Some("intent_b")
        );
        // scene_id=None → 回退首个 DeepExtracted（sc1）。
        assert_eq!(
            find_scene_intent(&graph, None, "intent_a").map(|i| i.intent_id.as_str()),
            Some("intent_a")
        );
        // 不跨场景兜底：当前场景没有就是没有，防把别处的 effect_policy 错绑。
        assert!(find_scene_intent(&graph, Some("sc2"), "intent_a").is_none());
    }

    // C7：盖章原语——roll_check / request_player_roll 共用；gate 结算靠这枚引用恢复绑定。
    #[test]
    fn stamp_scene_intent_sets_target_and_advice_ref() {
        let intent = SceneMechanicIntent {
            intent_id: "cut_cable".to_string(),
            description: "d".to_string(),
            tested_parameter: "brawling".to_string(),
            difficulty: Some(json!({"kind":"dv","value":13})),
            effect_policy: EffectPolicy::default(),
            source_anchor: "# Page 1".to_string(),
        };
        let mut contract = crate::gate::fixtures::gate_contract("c1");
        stamp_scene_intent(&mut contract, Some(&intent));
        assert!(matches!(contract.target, CheckTargetModel::StaticNumber { value: 13, .. }), "dv 13 must become the target: {:?}", contract.target);
        assert!(contract.advice_refs.iter().any(|r| r == "scene_mechanic:cut_cable"), "advice_refs must carry the stamp: {:?}", contract.advice_refs);
        // None → no-op（无意图调用零改动）。
        let before = crate::gate::fixtures::gate_contract("c2");
        let mut untouched = before.clone();
        stamp_scene_intent(&mut untouched, None);
        assert_eq!(untouched.advice_refs, before.advice_refs);
    }

    #[test]
    fn other_intent_folds_to_unexecutable_fact() {
        let raw = json!({"kind":"weird_custom_directive","detail":"unsupported"});
        let policy: EffectPolicy =
            serde_json::from_value(json!({"on_success":[raw.clone()],"on_failure":[]})).unwrap();
        let item = &select_intents(&policy, true)[0];
        assert!(matches!(item, EffectPatchIntent::Other(_)), "must deserialize as Other: {item:?}");
        match fold_unexecutable(item, "scene.policy test") {
            StatePatch::CreateFact { target, fact, reason } => {
                assert_eq!(target, "scene.policy");
                assert_eq!(fact.pointer("/unexecutable_intent"), Some(&raw), "fact must carry the original JSON");
                assert_eq!(reason, "scene.policy test");
            }
            other => panic!("expected CreateFact, got {other:?}"),
        }
    }
}
