//! GOLD live e2e（Phase 3 §6 黄金链 / 必修 must_fix 第二条）：坐实 **agent 路径**
//! 上游对抗语义预 pass → roll_check 工具注入 opposed → settle.rs 结算 → contest
//! meet_or_beat 读 NPC 现搓防御值 → 出真胜负。区别于 `trpg-contest` 的
//! `live_opposed_meet_or_beat`（自给自足直调 ContestService、绕过 agent 链）：本测
//! 真走 `opposed_prepass::prepare_binding`（语义判定攻击+现搓防御值）+ 真 `RollCheckTool`
//! （GM 漏填 opposed 由 ctx.opposed_binding 注入）+ 真 `settle_system_check` +
//! `execute_system_roll_bundle`（runtime 预掷防御骰）。这正是产品痛点（damage_packets=0
//! / NPC hp_current=NULL 全发生在 agent 路径）所在的链路。
//!
//! 需 DATABASE_URL 指向含 `cyberpunk_red`(meet_or_beat) kernel 的库(:54346) + LLM env
//! (TRPG_LLM_*, TRPG_NPC_PERSONA_SYNTHESIS=true)。缺则 SKIP。
//! 攻击意图检测用 MockLlm 确定性命中（预 pass 内部语义判定本就 fail-closed，
//! 真 LLM 命中率非本测点；本测点是"命中后整条 agent 链落账"），现搓防御值用真 LLM。
//!
//! Run:
//!   set -a; source .env; set +a
//!   DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54346/chatrpg \
//!   TRPG_NPC_PERSONA_SYNTHESIS=true \
//!   cargo test -p trpg-gm --test live_opposed_prepass_agent_path -- --nocapture
use async_trait::async_trait;
use futures_core::Stream;
use serde_json::{json, Value};
use std::pin::Pin;
use std::sync::Arc;
use trpg_db::Db;
use trpg_gm::ledger::TurnLedger;
use trpg_gm::opposed_prepass;
use trpg_gm::tools::{check::RollCheckTool, GmTool, ToolCtx};
use trpg_llm::LlmClient;
use trpg_model::*;
use trpg_params::{RuntimeActorParameters, RuntimeParameterService};
use trpg_runtime::RuntimeEngine;

const RULESET: &str = "cyberpunk_red";

/// MockLlm：complete_json 恒回攻击裁决（命中场景 NPC）——预 pass 的攻击意图检测点
/// 是确定性的（fail-closed 已单测覆盖）；现搓防御值走真 LLM（engine.ensure_npc_parameter
/// 内部自建 client，不经此 mock）。stream/with_tools 用 trait 默认实现（本测不触达）。
struct AttackVerdictLlm {
    target_npc_id: String,
}
#[async_trait]
impl LlmClient for AttackVerdictLlm {
    async fn complete_text(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<String> {
        unimplemented!("complete_text unused by prepass")
    }
    async fn complete_json(&self, _: Vec<ChatMessage>, _: f32) -> anyhow::Result<Value> {
        Ok(
            json!({"is_attack": true, "target_npc_id": self.target_npc_id, "reason": "fires at the scav"}),
        )
    }
    async fn stream_chat(
        &self,
        _: Vec<ChatMessage>,
        _: f32,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<String>> + Send>>> {
        unimplemented!("stream_chat unused by prepass")
    }
}

fn seed_actor(
    session: &str,
    actor_id: &str,
    kind: ActorKind,
    mech: Value,
) -> RuntimeActorParameters {
    RuntimeActorParameters {
        actor_param_id: format!("ap_{}", uuid::Uuid::new_v4().simple()),
        session_id: session.into(),
        actor_id: actor_id.into(),
        actor_kind: kind,
        ruleset_id: RULESET.into(),
        source_kind: "test_seed".into(),
        template_id: None,
        display_name: None,
        sheet_json: json!({}),
        mechanical_profile: mech,
        status_json: json!({}),
        visibility: Visibility::GmOnly,
        created_at_tick: Some(0),
        updated_at_tick: Some(0),
    }
}

#[tokio::test]
async fn agent_path_prepass_injects_opposed_and_npc_defense_drives_verdict() {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP: DATABASE_URL unset");
            return;
        }
    };
    let db = match Db::connect(&url).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("SKIP: connect failed: {e}");
            return;
        }
    };
    // 闸门:库内必须有 cyberpunk_red(meet_or_beat) kernel + homecoming 模组图谱。
    let kernel = db.load_rule_kernel(RULESET).await.ok().flatten();
    let is_mob = kernel
        .as_ref()
        .and_then(|k| k.dice_core.get("compare").and_then(|v| v.as_str()))
        == Some("meet_or_beat");
    if !is_mob {
        eprintln!("SKIP: no cyberpunk_red meet_or_beat kernel in this DB (need :54346)");
        return;
    }
    let module_id = "cyberpunk_red.homecoming";
    let graph = match db.load_module_graph(module_id).await.ok().flatten() {
        Some(g) => g,
        None => {
            eprintln!("SKIP: no homecoming module graph in this DB");
            return;
        }
    };
    // 找一个含战斗 NPC 的 deep-extracted 场景（scene_05 有 npc_scavvers）。
    let scene = graph
        .scenes
        .iter()
        .find(|s| s.referenced_npc_ids.iter().any(|id| id.contains("scav")));
    let scene = match scene {
        Some(s) => s,
        None => {
            eprintln!("SKIP: no combat scene with a scav NPC");
            return;
        }
    };
    let target_npc_id = scene
        .referenced_npc_ids
        .iter()
        .find(|id| id.contains("scav"))
        .unwrap()
        .clone();
    println!("[scene] {} target_npc={}", scene.node_id, target_npc_id);

    // 自给自足 seed：临时 session（绑模组+场景），攻击方 pc.current Handgun=14。跑完即弃。
    let session = format!("session_prepass_e2e_{}", uuid::Uuid::new_v4().simple());
    let params = RuntimeParameterService::new(db.clone());
    params
        .upsert_actor_parameters(&seed_actor(
            &session,
            "pc.current",
            ActorKind::PlayerCharacter,
            json!({"stats": {"REF": 6}, "skills": {"Handgun": 14}}),
        ))
        .await
        .expect("seed pc");
    // 防御方 NPC 卡必须存在，ensure_npc_parameter 才能 load 后合成（actor_id = 真实 graph id）。
    params
        .ensure_actor_parameters(&session, RULESET, &target_npc_id, ActorKind::Npc, 0)
        .await
        .expect("ensure npc card");

    let engine = RuntimeEngine::new(db.clone());
    let request = ContextRequest {
        ruleset_id: RULESET.into(),
        module_id: Some(module_id.into()),
        session_id: session.clone(),
        turn_id: format!("turn_{}", uuid::Uuid::new_v4().simple()),
        viewer: VisibilityProfile::gm(),
        token_budget: TokenBudget::default(),
    };
    let state = RuntimeState {
        ruleset_id: RULESET.into(),
        module_id: Some(module_id.into()),
        scene_id: Some(scene.node_id.clone()),
        ..Default::default()
    };

    // —— ① 上游对抗预 pass（真 prepare_binding）：语义判定攻击命中（MockLlm 确定性）
    //    → attack_defense_param 按 compare 取 stats.defense → 真 LLM 现搓防御值落卡。
    let llm: Arc<dyn LlmClient> = Arc::new(AttackVerdictLlm {
        target_npc_id: target_npc_id.clone(),
    });
    let binding = opposed_prepass::prepare_binding(
        &engine,
        &llm,
        &request,
        &state,
        "我拔枪朝那个清道夫开火，瞄准胸口！",
        None,
        &[],
    )
    .await;
    let binding = match binding {
        Some(b) => b,
        None => {
            eprintln!("SKIP: prepass produced no binding (synthesis off / no LLM / no card) — set TRPG_NPC_PERSONA_SYNTHESIS=true + TRPG_LLM_*");
            return;
        }
    };
    assert_eq!(
        binding.persona.actor_id, target_npc_id,
        "binding 必须绑真实场景 NPC id（非 npc.opposition 占位符）"
    );
    assert_eq!(
        binding.opponent_parameter, "defense",
        "CPR meet_or_beat → 防御键=defense（check_param_need 数据映射）"
    );
    println!(
        "[prepass] bound npc={} param={}.{}",
        binding.persona.actor_id, binding.bucket, binding.opponent_parameter
    );
    // B1 断言：现搓的 defense 已投影进 NPC mechanical_profile（contest 读 mech）。
    let npc = params
        .load_actor_parameters(&session, &target_npc_id)
        .await
        .ok()
        .flatten()
        .expect("npc card");
    assert!(
        npc.mechanical_profile.pointer("/stats/defense").is_some(),
        "现搓 defense 必须投影进 mechanical_profile，否则 contest 读不到"
    );

    // —— ② agent 路径 roll_check 工具：GM 漏填 opposed（args 无 opposed），由 ctx.opposed_binding
    //    注入（inject_opposed_binding）→ stamp_opposed_check → settle_system_check →
    //    execute_system_roll_bundle（runtime 预掷防御骰）→ contest meet_or_beat 读 NPC defense。
    let ctx = ToolCtx {
        engine: &engine,
        request: &request,
        state: &state,
        scene_extractor: None,
        obligations: None,
        data_dir: None,
        current_mode: None,
        opposed_binding: Some(&binding),
        nominated_reveals: None,
        rejected_nominations: None,
    };
    let mut ledger = TurnLedger::new();
    // 注意：GM agent 调 roll_check **不传 opposed**（这正是产品痛点：5/5 次漏填）。
    let args = json!({"check_label": "Handgun attack on the scavenger", "tested_parameter": "Handgun", "visibility": "public"});
    let out = RollCheckTool
        .call(&ctx, &mut ledger, args)
        .await
        .expect("roll_check must settle (not error)");
    let result = out.result;
    println!(
        "[roll_check settled] {}",
        serde_json::to_string(&result).unwrap()
    );

    // —— ③ 断言整条 agent 链：契约带上了对抗参数 + contest 出真胜负 + 消费了 NPC defense。
    let contract = ledger
        .snapshot()
        .check_contracts
        .last()
        .cloned()
        .expect("contract recorded in ledger");
    assert!(
        contract.target_actor.is_some(),
        "预 pass 注入后契约必须带 target_actor（对抗形态）"
    );
    assert_eq!(
        contract.target_actor.as_ref().unwrap().actor_id,
        target_npc_id,
        "target_actor=真实场景 NPC id"
    );
    assert_eq!(
        contract
            .opponent_tested_parameter
            .as_ref()
            .map(|p| p.key.as_str()),
        Some("defense"),
        "opponent_tested_parameter=defense"
    );

    let outcome = &result["outcome"];
    let model_kind = outcome
        .pointer("/resolution_model/kind")
        .and_then(|v| v.as_str());
    assert_eq!(
        model_kind,
        Some("opposed_roll"),
        "meet_or_beat 对抗契约必须经 agent 链建 OpposedRoll，实际={:?}",
        model_kind
    );
    let success = outcome.get("success").and_then(|v| v.as_bool());
    assert!(
        success.is_some(),
        "agent 链对抗必须出胜负（success != null），而非 Provisional-null（消费层断）"
    );
    let dv = outcome
        .pointer("/opposed/defender_value")
        .and_then(|v| v.as_i64());
    assert!(
        dv.is_some(),
        "outcome.opposed.defender_value 必须非空（= 消费了 NPC 现搓的 defense）"
    );
    let av = outcome
        .pointer("/opposed/attacker_value")
        .and_then(|v| v.as_i64());
    assert_eq!(av, Some(14), "attacker_value 应为 pc.current 的 Handgun=14");
    println!("PASS: agent 路径 prepass 注入 opposed → contest 读 NPC defense={:?}，attacker(14) vs defender，success={:?}", dv, success);

    // 清场：删临时 session 的 actor 参数（留库干净）。
    sqlx::query("delete from runtime_actor_parameters where session_id=$1")
        .bind(&session)
        .execute(&db.pool)
        .await
        .ok();
}
