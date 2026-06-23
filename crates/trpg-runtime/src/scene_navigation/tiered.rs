//! R5 Task1a：场景导航的 critical/heavy 拆分。
//! `scene_navigate_critical`（决策 + set_session_scene + SceneChanged，同步）/
//! `scene_navigate_heavy`（到场深抽 + frontier，后台）/ `scene_navigator`（薄 wrapper，
//! critical→heavy 串行，字节等价旧逻辑）。复用父模块的 build_nav_prompt / validate_transition /
//! extract_module_scenes / prefetch_frontier / SCENE_NAV_SYS。
use super::{
    build_frontier_block, build_nav_exits, build_nav_prompt, build_nav_prompt_with_exits,
    extract_module_scenes, gravity_nav_system_prompt, nav_content_gravity_enabled,
    nav_departure_commit_enabled, nav_follow_flow_links_enabled, nav_objective_commit_enabled,
    prefetch_frontier, resolve_offgraph_to_neighbor, validate_transition,
    with_flow_link_clause, with_frontier_focus_clause, SCENE_NAV_SYS,
};
use crate::progression::{
    augment_program_with_spine, compute_frontier, derive_threat_objective, evaluate,
    program_from_module_graph, progression_engine_enabled, replay_domain_events, ProgressEvent,
};
use serde_json::json;
use tracing::info;
use trpg_db::Db;
use trpg_llm::LlmClient;
use trpg_model::{Visibility, WorldEventKind};
use trpg_time::WorldTimeService;

/// R5 critical 切场景的产出：成功切换才 Some，携带新 target 供 heavy 深抽。
/// from/reason 供调用方折成 SceneTransition 事件（execute.rs）。
#[derive(Debug, Clone)]
pub struct SceneNavCommit {
    pub from: String,
    pub to: String,
    pub reason: String,
}

/// R5 critical 半：语义判定 + set_session_scene + SceneChanged world event。**不**深抽/
/// 前探（那是 heavy，下一回合 prepare_turn_context 不强依赖——SkeletonOnly 降级块兜底）。
/// 全程 fail-closed：无图/空图/LLM 失败/校验不过 → Ok(None)（留原场景，不乱跳、不编造）。
/// 返回 Some(commit) 仅当真切换；调用方据此发 SceneTransition 事件 + 排 heavy 深抽。
pub async fn scene_navigate_critical(
    db: &Db,
    llm: &dyn LlmClient,
    session_id: &str,
    module_id: &str,
    _data_dir: &std::path::Path,
    player_input: &str,
    narration: &str,
) -> anyhow::Result<Option<SceneNavCommit>> {
    let Some(graph) = db.load_module_graph(module_id).await? else {
        return Ok(None);
    };
    if graph.scenes.is_empty() {
        return Ok(None);
    }
    let current = db.load_session_scene(session_id).await?.unwrap_or_default();
    let list = graph
        .scenes
        .iter()
        .map(|s| format!("{} | {} | {}", s.node_id, s.node_type, s.title))
        .collect::<Vec<_>>()
        .join("\n");
    let cur_node = graph.scenes.iter().find(|s| s.node_id == current);
    let cur_title = cur_node.map(|s| s.title.as_str()).unwrap_or("(未定)");
    // L-C content-gravity（理念§4/§7，flag 默认 ON / OFF 字节等价）：把当前场景的真实直接
    // 衔接 beat（去重、in-graph、≠current）作为软推进上下文喂给导航器，并换用追加了「软推进」
    // 许可的 system prompt。OFF ⇒ 走原 SCENE_NAV_SYS + 裸 build_nav_prompt（提示串字节等价基线）。
    let gravity = nav_content_gravity_enabled();
    let (mut sys, mut usr) = if gravity {
        // J3 FLOW-LINK CONSUMER（TRPG_NAV_FOLLOW_FLOW_LINKS，默认 OFF）：exits 现经
        // build_nav_exits 构建——flag ON 时已授权有向脊边（sequential/trigger/branch+anchor）
        // 排在 spatial 桥之前并标注其 link_type；flag OFF ⇒ 与历史内联逐字节一致（OFF==baseline）。
        let follow_flow = nav_follow_flow_links_enabled();
        let exits = build_nav_exits(cur_node, &graph.scenes, &current, follow_flow);
        // L-Y/L-AA：gravity 串在 ①②③ 之上按各自 flag 追加④离场提交+⑤目标承接（皆默认 ON）；
        // ⑥流转脊优先（with_flow_link_clause）再按本 flag 叠加（仅玩家驱动才 commit 脊边、反铁路
        // fail-closed）。三 flag 皆退回基线 ⇒ 纯 SCENE_NAV_SYS_GRAVITY 字节等价 L-C 基线。
        (
            with_flow_link_clause(
                gravity_nav_system_prompt(
                    nav_departure_commit_enabled(),
                    nav_objective_commit_enabled(),
                ),
                follow_flow,
            ),
            build_nav_prompt_with_exits(&current, cur_title, &exits, &list, player_input, narration),
        )
    } else {
        (
            SCENE_NAV_SYS.to_string(),
            build_nav_prompt(&current, cur_title, &list, player_input, narration),
        )
    };
    // ADVANCEMENT-FRONTIER CONSUMER (TRPG_PROGRESSION_ENGINE, 默认 OFF)：用本 session 的
    // DomainEvents 重建运行时 ProgressionState，seed 当前场景，把**合法 frontier** 喂给导演（仅
    // 在其中选焦/搬内容，反铁路绝不 teleport）。OFF ⇒ 整块跳过 ⇒ sys/usr 字节等价基线。
    // 全程 fail-closed：取事件失败/空 ⇒ 空 frontier ⇒ 不追加（绝不乱跳、绝不编造）。
    if progression_engine_enabled() {
        let events = db.list_domain_events(session_id, 5000).await.unwrap_or_default();
        let mut program = program_from_module_graph(&graph);
        // PL-1 (PHASE 2): give scene_01 an OPEN threat objective so the frontier is
        // non-empty even before any flow ridge fires (the proven scene_01 starvation:
        // frontier_total=0 for t01–t14). Vocabulary is source-grounded from the
        // module's OWN matcher config (npc bindings + technical-option affordances) —
        // ZERO ruleset/module name branching; fail-closed (no config / blank vocab ⇒
        // no objective, never invent). The GM's free-form fact_id is aligned via the
        // same case-insensitive-substring scheme the module config already uses
        // (PredicateExpr::AnyFactMatches — Rust evaluates the guard, the LLM never).
        if let Some(cfg) = db.load_module_config(module_id).await {
            let mut vocab: Vec<String> = Vec::new();
            for b in &cfg.npc_actor_bindings {
                vocab.extend(b.matcher.iter().cloned());
            }
            if let Some(rows) = &cfg.technical_option_table {
                for row in rows {
                    vocab.extend(row.matcher.iter().cloned());
                }
            }
            if let Some(d) = derive_threat_objective(
                "obj.neutralize_threat",
                &vocab,
                "rev.threat_outcome",
                "module_config: npc_actor_bindings + technical_option_table matchers",
            ) {
                program.objectives.push(d.objective);
                program.rules.push(d.outcome_rule);
            }
        }
        // PL-5 (PHASE 2 ROUND-2): generalize the per-scene objective/out-edge from
        // scene_01 to the WHOLE spine — each spine scene gets a structural
        // `Entered→Activate(next)` rule (frontier non-empty everywhere, not just
        // scene_01) plus a scene-scoped open objective gated on its own vocab. Pure
        // structure (page order + each scene's tokens): ZERO ruleset/scene-name
        // branching; fail-closed (blank vocab ⇒ no objective). Skips ridges already
        // authored by `program_from_module_graph`. This is the ROUND-2 fix for the
        // proven "froze at scene_02 (no objective/out-edge)" 1-hop stall.
        augment_program_with_spine(&mut program, &graph);
        let (mut state, _hist) = replay_domain_events(&events, &program.borrow());
        // 焦点此刻就在当前场景（权威 session 状态，非猜测）：seed Entered(current)，让从它出发
        // 的已授权 ridge 填充 frontier。
        let step_signals = if current.trim().is_empty() {
            Vec::new()
        } else {
            evaluate(
                &mut state,
                &[ProgressEvent::Entered(current.clone())],
                &program.borrow(),
            )
        };
        let frontier = compute_frontier(&state, &program.objectives, &program.trackers);
        let block = build_frontier_block(&frontier, &graph.scenes);
        if !block.is_empty() {
            usr.push_str("\n\n");
            usr.push_str(&block);
        }
        sys = with_frontier_focus_clause(sys, true);
        info!(
            session_id,
            frontier_active = frontier.active_units.len(),
            frontier_total = frontier.len(),
            frontier_objectives = frontier.open_objectives.len(),
            signals = step_signals.len(),
            program_rules = program.rules.len(),
            program_objectives = program.objectives.len(),
            "progression frontier computed (engine ON)"
        );
    }
    // EV-2 EXACT-EVIDENCE PROJECTOR (TRPG_EXACT_EVIDENCE_PROJECTOR_V1 / master
    // TRPG_PROGRESS_EVIDENCE_V1, default OFF): project this session's committed
    // DomainEvents into an AcceptedEvidence ledger via the deterministic Rust
    // exact path (no LLM) — an authored-clue PlayerLearnedFact/FactRevealed whose
    // fact_id resolves to a catalog atom AND whose knowledge_state is player-known-
    // true. fail-closed (synthetic wf_chk_* / non-true belief / unresolved ref all
    // skip). The engine does NOT consume the ledger yet (EV-6); this block only
    // builds + logs it. OFF ⇒ block skipped entirely (no prompt mutation, no event,
    // no RNG) ⇒ sys/usr byte-identical baseline.
    if crate::evidence_projection::exact_evidence_projector_enabled() {
        let events = db.list_domain_events(session_id, 5000).await.unwrap_or_default();
        let catalog = crate::evidence_projection::build_evidence_atom_catalog(&graph);
        let ledger = crate::evidence_projection::project_exact_evidence(&events, &catalog);
        info!(
            session_id,
            catalog_atoms = catalog.len(),
            domain_events = events.len(),
            accepted_evidence = ledger.len(),
            "exact-evidence projector computed (EV-2 ON; engine does not consume yet)"
        );
    }
    // EV-3 PROGRESS OFFERS (TRPG_PROGRESS_OFFERS_V1 / master TRPG_PROGRESS_EVIDENCE_V1,
    // default OFF): emit a machine-readable EvidenceOfferSet = the EV-2 catalog ∩ the
    // current scene's SURFACED clue atoms (conservative — never the whole catalog),
    // and additively append those capabilities' human-readable meanings to the GM
    // prompt (the GM is shown "you MAY note cap_X = <meaning> if <basis>"). Each
    // cap_id is a turn-scoped OPAQUE handle (≠ atom_id, ≠ objective_id). The machine
    // OfferSet is held turn-local for EV-4; no claims are parsed and the engine is
    // unchanged here. fail-closed (no surfaced clue / unresolved ref ⇒ no offer).
    // OFF ⇒ block skipped entirely (no OfferSet, no prompt mutation, no event, no
    // RNG) ⇒ sys/usr byte-identical baseline; even ON with an empty set the prompt is
    // unchanged (the append is guarded on a non-empty rendered block).
    if crate::evidence_offers::progress_offers_enabled() {
        // Surfaced clues come from page-containment projection (CL-1). Run it on a
        // CLONE so the offer derivation never mutates the graph the other blocks read.
        let mut og = graph.clone();
        let _ = crate::clue_projection::project_clues_onto_scenes(&mut og);
        let catalog = crate::evidence_projection::build_evidence_atom_catalog(&og);
        let turn_num = db.count_session_turns(session_id).await.unwrap_or(0);
        let turn_id = format!("turn_{turn_num}");
        let offer_set =
            crate::evidence_offers::derive_offer_set(&og, &catalog, &current, session_id, &turn_id);
        let block = crate::evidence_offers::render_offer_prompt_block(&offer_set);
        if !block.is_empty() {
            usr.push_str("\n\n");
            usr.push_str(&block);
        }
        info!(
            session_id,
            turn_id = %turn_id,
            catalog_atoms = catalog.len(),
            offers = offer_set.len(),
            prompt_appended = !block.is_empty(),
            "progress offers computed (EV-3 ON; GM not consuming claims yet)"
        );
    }
    // EV-4 PROGRESS CLAIMS — SHADOW, PREP HALF (TRPG_PROGRESS_CLAIMS_SHADOW_V1 /
    // master TRPG_PROGRESS_EVIDENCE_V1, default OFF): close the GM↔Rust claim loop in
    // shadow. We re-derive THIS turn's EvidenceOfferSet (deterministic; same inputs as
    // the EV-3 block) + catalog, and additively append the short instruction telling
    // the GM it MAY return a `progress_claims` sidecar for a capability that genuinely
    // occurred. The OfferSet + catalog are held across the LLM call for the admission
    // half. OFF ⇒ block skipped entirely (no prompt mutation, no derivation, no RNG)
    // ⇒ sys/usr byte-identical baseline.
    let ev4_admission: Option<(
        trpg_model::adventure_ir::EvidenceOfferSet,
        trpg_model::adventure_ir::EvidenceAtomCatalog,
        String,
    )> = if crate::evidence_gateway::progress_claims_shadow_enabled() {
        let mut og = graph.clone();
        let _ = crate::clue_projection::project_clues_onto_scenes(&mut og);
        let catalog = crate::evidence_projection::build_evidence_atom_catalog(&og);
        let turn_num = db.count_session_turns(session_id).await.unwrap_or(0);
        let turn_id = format!("turn_{turn_num}");
        let offer_set =
            crate::evidence_offers::derive_offer_set(&og, &catalog, &current, session_id, &turn_id);
        usr.push_str("\n\n");
        usr.push_str(&crate::evidence_gateway::render_claim_instruction());
        Some((offer_set, catalog, turn_id))
    } else {
        None
    };
    let decision = match llm
        .complete_json(vec![trpg_llm::system(&sys), trpg_llm::user(&usr)], 0.0)
        .await
    {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "scene_navigate_critical llm failed; stay");
            return Ok(None);
        }
    };
    // EV-4 PROGRESS CLAIMS — SHADOW, ADMISSION HALF: parse the GM's `progress_claims`
    // sidecar from the decision (closed schema, fail-closed), then run each claim
    // through the EvidenceGateway against this turn's OfferSet + committed DomainEvents
    // + catalog. **Shadow**: every admission decision is only LOGGED — the engine is
    // NOT called and no objective is completed (J3 unchanged). Rust resolves cap→atom
    // from the OfferSet; the LLM never names the atom.
    if let Some((offer_set, catalog, turn_id)) = ev4_admission {
        let claims = crate::evidence_gateway::parse_progress_claims(&decision);
        if !claims.is_empty() {
            let events = db.list_domain_events(session_id, 5000).await.unwrap_or_default();
            let mut ledger = trpg_model::adventure_ir::EvidenceLedger::new();
            let mut admitted = 0usize;
            for claim in &claims {
                let inp = crate::evidence_gateway::GatewayInputs {
                    session_id,
                    turn_id: &turn_id,
                    offer_set: &offer_set,
                    committed_events: &events,
                    catalog: &catalog,
                    ledger: &ledger,
                };
                match crate::evidence_gateway::EvidenceGateway::admit(claim, &inp) {
                    Ok(ev) => {
                        info!(
                            session_id,
                            turn_id = %turn_id,
                            cap = %claim.cap_id.as_str(),
                            atom = %ev.atom_id.as_str(),
                            authority = "GmWitnessed",
                            "EV-4 claim ADMITTED (shadow; engine not consuming, J3 unchanged)"
                        );
                        ledger.append(ev);
                        admitted += 1;
                    }
                    Err(reason) => {
                        info!(
                            session_id,
                            turn_id = %turn_id,
                            cap = %claim.cap_id.as_str(),
                            reason = reason.as_str(),
                            "EV-4 claim REJECTED (shadow)"
                        );
                    }
                }
            }
            info!(
                session_id,
                turn_id = %turn_id,
                claims = claims.len(),
                admitted,
                ledger = ledger.len(),
                "EV-4 shadow admission complete (engine not consuming; J3 unchanged)"
            );
        }
    }
    let target = match validate_transition(&decision, &graph.scenes, &current) {
        Some(t) => t,
        None => {
            // MAT.M9c（DP-C）：LLM 想移动但目标 off-graph → Enforce 下纠回当前场景真实邻接
            // （绝不去图谱外编造场景）；匹配不到 ⇒ 留原场景（既有 fail-closed 行为）。
            // Off/Shadow ⇒ resolve_offgraph_to_neighbor 恒 None ⇒ 字节级基线。
            let mode = trpg_model::MaterializationAffordanceMode::from_env();
            match resolve_offgraph_to_neighbor(&decision, &graph.scenes, &current, mode) {
                Some(t) => {
                    info!(session_id, from = %current, neighbor = %t, "MAT.M9c: off-graph nav target remapped to in-graph neighbor");
                    t
                }
                None => return Ok(None),
            }
        }
    };
    db.set_session_scene(session_id, &target).await?;
    let reason = decision
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    info!(session_id, from = %current, to = %target, %reason, "scene transition (critical)");
    // 写 scene_transition world event（kind/from/to/reason），best-effort 失败只 warn。
    let event_data = json!({"kind": "scene_transition", "from": current, "to": target, "reason": reason, "module_id": module_id});
    if let Err(err) = WorldTimeService::new(db.clone())
        .record_event(
            session_id,
            None,
            None,
            WorldEventKind::SceneChanged,
            event_data,
            Visibility::GmOnly,
        )
        .await
    {
        tracing::warn!(error = %err, "scene_navigate_critical: world event write failed; scene already switched");
    }
    Ok(Some(SceneNavCommit {
        from: current,
        to: target,
        reason,
    }))
}

/// R5 heavy 半：到场深抽 + frontier 前探（best-effort，后台跑，下一回合不强依赖）。
/// 全程 fail-closed：深抽/前探失败只 warn，绝不回滚已由 critical 切换的场景。
pub async fn scene_navigate_heavy(
    db: &Db,
    llm: &dyn LlmClient,
    module_id: &str,
    target: &str,
    data_dir: &std::path::Path,
) {
    // 到场深抽（目标若 SkeletonOnly）；已预抽则内部判定 0、无害。source_id=None →
    // extract_module_scenes 从 bundle.source_index 推导。失败不回滚切换（场景已更）。
    if let Err(err) =
        extract_module_scenes(db, llm, module_id, None, None, data_dir, 12, Some(target)).await
    {
        tracing::warn!(error = %err, %target, "on-arrival deep-extract failed; scene already switched");
    }
    // Frontier 前探一跳（best-effort）：target 深抽后其出口 links 已写回 bundle，重新
    // load_module_graph 取 target 的出口 to_node_id，对每个仍 SkeletonOnly 的出口（去重、
    // bounded 上限）逐个 only=Some 深抽。玩家移动到衔接场景时即时；没去的场景永停 stub。
    prefetch_frontier(db, llm, module_id, target, data_dir).await;
}

/// 旧入口（critical→heavy 串行一体）：用一次 LLM 判定党是否移动到模组某真实场景，
/// 校验通过则更新 `sessions.current_scene_id`、写 `WorldEventKind::SceneChanged` world
/// event，再对目标场景到场深抽（若仍 SkeletonOnly）+ frontier 前探。
///
/// R5 拆分后保留为薄 wrapper（critical 后串 heavy），行为字节等价旧逻辑，供尚未拆分的
/// 调用方与单 transport 临时复用；新执行器（execute.rs）走拆分路径（critical 同步、heavy
/// 后台）。全程 fail-closed：取不到图/空图/LLM 失败/校验不过 → Ok(())（留原场景）。
pub async fn scene_navigator(
    db: &Db,
    llm: &dyn LlmClient,
    session_id: &str,
    module_id: &str,
    data_dir: &std::path::Path,
    player_input: &str,
    narration: &str,
) -> anyhow::Result<()> {
    if let Some(commit) = scene_navigate_critical(
        db,
        llm,
        session_id,
        module_id,
        data_dir,
        player_input,
        narration,
    )
    .await?
    {
        scene_navigate_heavy(db, llm, module_id, &commit.to, data_dir).await;
    }
    Ok(())
}
