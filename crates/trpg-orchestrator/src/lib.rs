use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use trpg_db::Db;
use trpg_interaction::InteractionLifecycleKernel;
use trpg_model::*;
use trpg_semantics::{NeedSignal, SemanticIntentService, SemanticNeedClassifier};
use trpg_time::WorldTimeService;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateRelation {
    NoActiveGate,
    RollReply,
    ChoiceReply,
    SupersedingIntent,
    UnrelatedNewAction,
    Ambiguous,
}

impl GateRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoActiveGate => "no_active_gate",
            Self::RollReply => "roll_reply",
            Self::ChoiceReply => "choice_reply",
            Self::SupersedingIntent => "superseding_intent",
            Self::UnrelatedNewAction => "unrelated_new_action",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnRouteKind {
    ResolveGateFirst,
    SupersedeGateThenStartFrame,
    SupersedeGateThenContinueFrame,
    SupersedeGateThenObjectInteraction,
    StartFrameFirst,
    ContinueFrameFirst,
    ObjectInteractionFirst,
    AbilityActivationFirst,
    DirectorFirst,
    StandaloneCheckOrAgent,
    LlmNarration,
}

impl TurnRouteKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ResolveGateFirst => "resolve_gate_first",
            Self::SupersedeGateThenStartFrame => "supersede_gate_then_start_frame",
            Self::SupersedeGateThenContinueFrame => "supersede_gate_then_continue_frame",
            Self::SupersedeGateThenObjectInteraction => "supersede_gate_then_object_interaction",
            Self::StartFrameFirst => "start_frame_first",
            Self::ContinueFrameFirst => "continue_frame_first",
            Self::ObjectInteractionFirst => "object_interaction_first",
            Self::AbilityActivationFirst => "ability_activation_first",
            Self::DirectorFirst => "director_first",
            Self::StandaloneCheckOrAgent => "standalone_check_or_agent",
            Self::LlmNarration => "llm_narration",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnIntent {
    pub intent_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub gate_relation: GateRelation,
    pub frame_relation: FrameRelation,
    pub action_kind: SituationActionKind,
    pub target_kind: String,
    pub object_interaction_kind: Option<String>,
    /// When the player explicitly named a rules check (e.g. "make a Sanity
    /// roll"), the parameter that check tests, as named by the semantic layer.
    /// Drives the named-check builder which binds tested_parameter to it.
    #[serde(default)]
    pub named_parameter: Option<String>,
    pub confidence: RulingConfidence,
    pub evidence_terms: Vec<String>,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRoute {
    pub route_kind: TurnRouteKind,
    pub should_resolve_gate_first: bool,
    pub prefer_conflict_first: bool,
    pub prefer_object_first: bool,
    pub prefer_ability_first: bool,
    pub force_director: bool,
    pub allow_agent_plan: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnOrchestrationResult {
    pub handled: bool,
    pub phases: Vec<String>,
    pub intent: TurnIntent,
    pub route: TurnRoute,
    pub active_frame_id: Option<String>,
    pub active_gate_id: Option<String>,
    pub active_pending_check_id: Option<String>,
    pub superseded_gate_id: Option<String>,
    pub superseded_pending_check_id: Option<String>,
    pub repairs: Vec<InvariantRepair>,
    pub world_tick: i64,
    /// P0-3: structured, semantically-derived need signal (rule/material/scene +
    /// confidence) projected off the turn's `SemanticIntentResult`. This is the
    /// keyword-free replacement for ad-hoc keyword scans; downstream retrieval
    /// can consume it instead of re-scanning the input for module nouns.
    #[serde(default = "need_signal_default")]
    pub need_signal: NeedSignal,
}

fn need_signal_default() -> NeedSignal {
    NeedSignal::NONE
}

#[derive(Debug, Clone, Copy)]
pub struct TurnOrchestratorInput<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub ruleset_id: &'a str,
    pub module_id: Option<&'a str>,
    pub user_input: &'a str,
}

#[derive(Clone)]
pub struct TurnOrchestrator {
    pub db: Db,
}

impl TurnOrchestrator {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn reduce_turn(
        &self,
        input: TurnOrchestratorInput<'_>,
    ) -> Result<TurnOrchestrationResult> {
        let time = WorldTimeService::new(self.db.clone())
            .ensure_session_time(input.session_id, Some(input.session_id))
            .await?;
        let lifecycle = InteractionLifecycleKernel::new(self.db.clone());
        let reconcile = lifecycle
            .reconcile_session(input.session_id)
            .await
            .unwrap_or_default();
        let repairs = reconcile.repairs;
        let open_gate = self
            .db
            .get_open_interaction_gate(input.session_id)
            .await
            .ok()
            .flatten();
        let pending = self
            .db
            .get_open_pending_check(input.session_id)
            .await
            .ok()
            .flatten();
        let active_frame = self
            .db
            .list_active_state_frames(input.session_id, 1)
            .await
            .ok()
            .and_then(|mut v| v.pop());
        let semantic = SemanticIntentService::from_env(self.db.clone())
            .classify_turn(SemanticIntentRequest {
                session_id: input.session_id.into(),
                turn_id: input.turn_id.into(),
                ruleset_id: input.ruleset_id.into(),
                module_id: input.module_id.map(str::to_string),
                active_frame_summary: active_frame
                    .as_ref()
                    .map(|f| serde_json::to_value(f).unwrap_or_else(|_| json!({})))
                    .unwrap_or_else(|| json!(null)),
                active_gate_summary: open_gate
                    .as_ref()
                    .map(|g| serde_json::to_value(g).unwrap_or_else(|_| json!({})))
                    .unwrap_or_else(|| json!(null)),
                player_input: input.user_input.into(),
                visible_scene_objects: json!({}),
                actor_parameter_summary: json!({}),
            })
            .await
            .ok();
        let intent = classify_turn_intent(
            input,
            active_frame.as_ref(),
            open_gate.as_ref(),
            pending.as_ref(),
            semantic.as_ref(),
        );
        // P0-3: derive the structured need signal from the SAME semantic result —
        // a keyword-free projection (see SemanticNeedClassifier) surfaced for
        // downstream retrieval to consume instead of scanning input for nouns.
        let need_signal = SemanticNeedClassifier::classify_opt(semantic.as_ref());
        let mut route = route_for(
            &intent,
            active_frame.as_ref(),
            open_gate.as_ref(),
            pending.as_ref(),
        );
        let mut superseded_gate_id = None;
        let mut superseded_pending_check_id = None;

        if matches!(
            intent.gate_relation,
            GateRelation::SupersedingIntent | GateRelation::UnrelatedNewAction
        ) && open_gate.is_some()
            && !route.should_resolve_gate_first
        {
            if let Some(gate) = open_gate.as_ref() {
                let reason = match route.route_kind {
                    TurnRouteKind::SupersedeGateThenStartFrame => {
                        SupersededReason::NewFrame.as_str()
                    }
                    TurnRouteKind::SupersedeGateThenContinueFrame => {
                        SupersededReason::TerminalIntent.as_str()
                    }
                    TurnRouteKind::SupersedeGateThenObjectInteraction => {
                        SupersededReason::TerminalIntent.as_str()
                    }
                    _ => SupersededReason::TerminalIntent.as_str(),
                };
                lifecycle
                    .supersede_gate_for_terminal_intent(gate, reason, input.user_input)
                    .await
                    .ok();
                superseded_gate_id = Some(gate.gate_id.clone());
                if let Some(p) = pending.as_ref() {
                    self.db
                        .supersede_pending_check(&p.check_id, reason, Some(time.world_tick))
                        .await
                        .ok();
                    superseded_pending_check_id = Some(p.check_id.clone());
                }
                route.reason = format!("{}; superseded active gate {}", route.reason, gate.gate_id);
            }
        }

        let result = TurnOrchestrationResult {
            handled: true,
            phases: vec!["turn_orchestrator".into(), "turn_route".into()],
            intent,
            route,
            active_frame_id: active_frame.as_ref().map(|f| f.frame_id.clone()),
            active_gate_id: open_gate.as_ref().map(|g| g.gate_id.clone()),
            active_pending_check_id: pending.as_ref().map(|p| p.check_id.clone()),
            superseded_gate_id,
            superseded_pending_check_id,
            repairs,
            world_tick: time.world_tick,
            need_signal,
        };
        let _ = WorldTimeService::new(self.db.clone())
            .record_event(
                input.session_id,
                Some(input.turn_id),
                result.active_frame_id.as_deref(),
                WorldEventKind::TableRuling,
                json!({"turn_orchestration": result.clone()}),
                Visibility::GmOnly,
            )
            .await;
        Ok(result)
    }
}

fn classify_turn_intent(
    input: TurnOrchestratorInput<'_>,
    active_frame: Option<&StateFrame>,
    gate: Option<&InteractionGate>,
    pending: Option<&PendingCheck>,
    semantic: Option<&SemanticIntentResult>,
) -> TurnIntent {
    let lower = input.user_input.to_lowercase();
    let mut evidence = Vec::new();
    let is_roll = looks_like_roll(&lower);
    let semantic_available = semantic.map(semantic_is_available).unwrap_or(false);
    let semantic_confident = semantic.map(semantic_is_confident).unwrap_or(false);
    let semantic_attack = semantic.map(semantic_is_combat_action).unwrap_or(false);
    let semantic_sustained = semantic
        .map(semantic_is_sustained_combat_action)
        .unwrap_or(false);
    let semantic_incoming = semantic.map(semantic_is_incoming_harm).unwrap_or(false);
    let semantic_exit = semantic.map(semantic_is_terminal_or_exit).unwrap_or(false);
    let semantic_object_action = semantic
        .map(semantic_is_object_interaction)
        .unwrap_or(false);
    let semantic_assessment = semantic.map(semantic_is_assessment_check).unwrap_or(false);
    let named_check_param = semantic.and_then(semantic_named_check_parameter);
    let allow_lexical_fallback = lexical_fallback_allowed(semantic_available, semantic_confident);

    let lexical_exit = contains_any(
        &lower,
        &[
            "撤退",
            "离开",
            "脱离",
            "逃",
            "停火",
            "和解",
            "讲和",
            "投降",
            "战斗结束",
            "结束战斗",
            "retreat",
            "withdraw",
            "leave",
            "escape",
            "ceasefire",
            "truce",
            "surrender",
            "stand down",
            "end combat",
        ],
    );
    let is_exit = semantic_exit || (allow_lexical_fallback && lexical_exit);

    let lexical_hypothetical_or_assessment = contains_any(
        &lower,
        &[
            "能不能",
            "可不可以",
            "是否",
            "判断",
            "分析",
            "评估",
            "安全吗",
            "安全切断",
            "可安全",
            "can i",
            "could i",
            "is it safe",
            "assess",
            "analyze",
            "evaluate",
        ],
    );
    let lexical_gm_secret_request = contains_any(
        &lower,
        &[
            "gm 暗中",
            "gm暗中",
            "暗中处理",
            "暗投",
            "秘密处理",
            "不要提醒",
            "secretly",
            "secret roll",
            "gm secretly",
        ],
    );
    let mentions_objectish = contains_any(
        &lower,
        &[
            "线",
            "线缆",
            "电缆",
            "cable",
            "wire",
            "门",
            "锁",
            "设备",
            "无人机",
            "drone",
            "object",
            "device",
            "lock",
        ],
    );
    let lexical_assessment_check =
        lexical_hypothetical_or_assessment && mentions_objectish && !lexical_gm_secret_request;
    let is_assessment_check =
        semantic_assessment || (allow_lexical_fallback && lexical_assessment_check);

    let lexical_attack_with_possessed_object = contains_any(
        &lower,
        &[
            "夺来的",
            "抢来的",
            "刚夺",
            "刚抢",
            "picked up",
            "grabbed",
            "taken",
        ],
    ) && contains_any(
        &lower,
        &["开火", "射击", "攻击", "打", "shoot", "fire", "attack"],
    );
    let lexical_object_action = is_actual_object_action(&lower)
        && !lexical_hypothetical_or_assessment
        && !lexical_gm_secret_request
        && !lexical_attack_with_possessed_object;
    // Disarm/grab/steal a weapon are BOTH combat maneuvers and object interactions;
    // the semantic classifier flags them combat by design, which must not erase the
    // possession-transfer object route. Gate on a grab/disarm verb + a weapon noun.
    let possession_maneuver = contains_any(
        &lower,
        &[
            "缴械",
            "打掉武器",
            "夺下武器",
            "夺刀",
            "夺枪",
            "下械",
            "disarm",
        ],
    ) || (contains_any(
        &lower,
        &["夺", "抢", "抓", "grab", "snatch", "steal", "wrest", "pry"],
    ) && contains_any(
        &lower,
        &[
            "枪", "武器", "刀", "剑", "weapon", "gun", "pistol", "knife", "blade", "rifle",
        ],
    ));
    let is_object_action = (semantic_object_action
        || (allow_lexical_fallback && lexical_object_action))
        && (!semantic_attack || possession_maneuver);

    let lexical_enemy_attack = contains_any(
        &lower,
        &[
            "朝我开火",
            "向我开火",
            "攻击我",
            "堵住我",
            "开始开火",
            "场面进入战斗",
            "under attack",
            "attacks me",
            "opens fire",
            "shoots at me",
            "blocks me",
        ],
    );
    let is_enemy_attack = semantic_incoming || (allow_lexical_fallback && lexical_enemy_attack);

    // Semantic-first combat classification. The lexical list is retained only as an audited fallback
    // for deployments without a working semantic classifier.  It is not the normal business route.
    let lexical_attack = contains_any(
        &lower,
        &[
            "开枪",
            "开火",
            "射击",
            "射他",
            "射它",
            "攻击",
            "还击",
            "拔枪",
            "打它",
            "打他",
            "猛打",
            "补枪",
            "补一枪",
            "再补一枪",
            "再来一枪",
            "再开一枪",
            "继续开火",
            "继续攻击",
            "继续打",
            "扣扳机",
            "扣动扳机",
            "砍",
            "shoot",
            "fire",
            "attack",
            "return fire",
            "keep firing",
            "another shot",
            "press the attack",
            "撃",
            "攻撃",
        ],
    );
    let is_attack = semantic_attack || (allow_lexical_fallback && lexical_attack);

    let lexical_continue = contains_any(
        &lower,
        &[
            "继续", "接着", "还是", "不停", "持续", "keep", "continue", "press",
        ],
    );
    let is_continue = semantic_sustained
        || (allow_lexical_fallback && lexical_continue)
        || (semantic_attack && active_frame.is_some());
    let is_director = contains_any(
        &lower,
        &[
            "观察",
            "看看",
            "不知道",
            "能做什么",
            "整理",
            "已知",
            "线索",
            "recap",
            "what can",
            "observe",
            "look",
        ],
    );
    // Explicit named check (e.g. "make a Sanity roll", "roll Spot Hidden"): the
    // semantic layer named a specific parameter. Wins over the generic assessment
    // route, but defers to frame/object/exit invariants so it never steals a
    // required reaction or an object interaction.
    let is_named_check = named_check_param.is_some()
        && !is_exit
        && !is_enemy_attack
        && !is_object_action
        && !is_attack;

    let mut gate_relation = if is_roll {
        GateRelation::RollReply
    } else if gate.is_none() {
        GateRelation::NoActiveGate
    }
    // Direction/stalemate gates are advisory. A clear semantic in-frame action
    // (continue pressure, attack, exit) supersedes the menu instead of being
    // treated as a menu choice. This keeps sustained combat moving.
    else if is_direction_gate(gate.unwrap()) && (is_continue || is_exit || is_attack) {
        GateRelation::SupersedingIntent
    } else if is_exit || is_object_action || is_enemy_attack || is_attack {
        GateRelation::SupersedingIntent
    } else if is_director || is_assessment_check || is_named_check {
        GateRelation::UnrelatedNewAction
    } else {
        GateRelation::Ambiguous
    };

    let mut named_parameter: Option<String> = None;
    let (mut frame_relation, mut action_kind, mut target_kind, mut object_kind) = if is_exit {
        evidence.push("exit_or_deescalation".into());
        (
            FrameRelation::ExitAttempt,
            if lower.contains("投降") || lower.contains("surrender") {
                SituationActionKind::Surrender
            } else if lower.contains("和解")
                || lower.contains("停火")
                || lower.contains("truce")
                || lower.contains("ceasefire")
            {
                SituationActionKind::Negotiate
            } else {
                SituationActionKind::Flee
            },
            "scene_or_opposition".into(),
            None,
        )
    } else if is_named_check {
        evidence.push("explicit_named_check".into());
        named_parameter = named_check_param.clone();
        (
            if active_frame.is_some() {
                FrameRelation::InsideFrameAction
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::InvestigateDuringConflict,
            "named_check".into(),
            None,
        )
    } else if is_assessment_check {
        evidence.push("technical_or_object_risk_assessment".into());
        (
            if active_frame.is_some() {
                FrameRelation::InsideFrameAction
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::InvestigateDuringConflict,
            "assessment_check".into(),
            None,
        )
    } else if is_enemy_attack {
        // Direct incoming attacks are frame/reaction events. Any ordinary object intent
        // in the same utterance (e.g. "it fires at me, I want to cut the cable") must be
        // deferred until the required reaction window is handled.
        evidence.push("enemy_initiated_conflict".into());
        (
            if active_frame.is_some() {
                FrameRelation::InsideFrameAction
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::EnemyInitiatedConflict,
            "opposition".into(),
            None,
        )
    } else if is_object_action {
        evidence.push("object_interaction".into());
        (
            if active_frame.is_some() {
                FrameRelation::InsideFrameAction
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::UseItem,
            "object".into(),
            Some(infer_object_interaction_kind(&lower)),
        )
    } else if is_attack {
        evidence.push("attack_or_continue_pressure".into());
        (
            if active_frame.is_some() {
                FrameRelation::InsideFrameAction
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::Attack,
            "opposition".into(),
            None,
        )
    } else if is_director {
        evidence.push("director_guidance".into());
        (
            if active_frame.is_some() {
                FrameRelation::PauseAndObserve
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::AskQuestion,
            "scene".into(),
            None,
        )
    } else {
        (
            if active_frame.is_some() {
                FrameRelation::InsideFrameAction
            } else {
                FrameRelation::OutsideFrameAction
            },
            SituationActionKind::Unknown,
            "unknown".into(),
            None,
        )
    };

    if let Some(sem) = semantic {
        let rust_has_strong_object = object_kind.is_some();
        let rust_has_assessment_check = target_kind == "assessment_check";
        let rust_has_named_check = target_kind == "named_check";
        let rust_has_frame_or_terminal = !evidence.is_empty()
            && (evidence.iter().any(|e| {
                e == "exit_or_deescalation"
                    || e == "enemy_initiated_conflict"
                    || e == "attack_or_continue_pressure"
            }));
        // LLM output is semantic evidence, but Rust route invariants remain the final owner.
        // In particular: disarm/grab, required-reaction gates, and technical assessments cannot be erased by a low/empty LLM classification.
        if sem.classifier == "semantic_unavailable_no_route" {
            evidence.push("semantic_unavailable_preserved_rust_route_invariant".into());
            gate_relation = parse_gate_relation(&sem.gate_relation).unwrap_or(gate_relation);
        } else if sem.confidence != RulingConfidence::Low
            || !sem.materialization_requests.is_empty()
        {
            if !rust_has_strong_object
                && !rust_has_assessment_check
                && !rust_has_named_check
                && !rust_has_frame_or_terminal
                && !is_roll
            {
                action_kind = sem.primary_action_kind;
                frame_relation = sem.frame_relation;
                gate_relation = parse_gate_relation(&sem.gate_relation).unwrap_or(gate_relation);
            }
            if sem
                .materialization_requests
                .iter()
                .any(|r| r.target_kind == RuleBindingTargetKind::AbilityDefinition)
                && !rust_has_strong_object
                && !rust_has_assessment_check
                && !rust_has_named_check
            {
                target_kind = "ability".into();
                action_kind = if matches!(
                    sem.primary_action_kind,
                    SituationActionKind::ActivateAbility | SituationActionKind::TriggerAbility
                ) {
                    sem.primary_action_kind
                } else {
                    SituationActionKind::ActivateAbility
                };
            }
            // v1.13.1: a materialization request for a weapon/object is not the same
            // as an object-interaction turn.  "用重型手枪开火" must remain Attack
            // while the pistol is materialized as the attacker's source object.  Only
            // promote semantic evidence to an object route when Rust has not already
            // identified a frame/terminal action and the semantic result names an
            // actual object interaction such as disarm, grab, pickup, or cut.
            if sem
                .materialization_requests
                .iter()
                .any(|r| r.target_kind == RuleBindingTargetKind::ObjectDefinition)
                && object_kind.is_none()
                && !rust_has_assessment_check
                && !rust_has_named_check
                && !rust_has_frame_or_terminal
            {
                if let Some(kind) = semantic_object_interaction_hint(sem) {
                    object_kind = Some(kind);
                    target_kind = "object".into();
                }
            }
            evidence.push(format!("semantic_classifier:{}", sem.classifier));
        }
    }

    TurnIntent {
        intent_id: format!("turn_intent_{}", Uuid::new_v4().simple()),
        session_id: input.session_id.into(),
        turn_id: input.turn_id.into(),
        gate_relation,
        frame_relation,
        action_kind,
        target_kind,
        object_interaction_kind: object_kind,
        named_parameter,
        confidence: if evidence.is_empty() {
            RulingConfidence::Low
        } else {
            RulingConfidence::Medium
        },
        evidence_terms: evidence,
        created_at: Utc::now(),
    }
}

fn route_for(
    intent: &TurnIntent,
    active_frame: Option<&StateFrame>,
    gate: Option<&InteractionGate>,
    _pending: Option<&PendingCheck>,
) -> TurnRoute {
    let has_gate = gate.is_some();
    let frame_starting = active_frame.is_none()
        && matches!(
            intent.action_kind,
            SituationActionKind::Attack
                | SituationActionKind::UnderAttack
                | SituationActionKind::EnemyInitiatedConflict
                | SituationActionKind::SceneEntersConflict
                | SituationActionKind::Defend
                | SituationActionKind::Counterattack
                | SituationActionKind::Hack
                | SituationActionKind::DisableDevice
                | SituationActionKind::Rescue
                | SituationActionKind::CastOrUsePower
        );
    let frame_continuing = active_frame.is_some()
        && matches!(
            intent.action_kind,
            SituationActionKind::Attack
                | SituationActionKind::UnderAttack
                | SituationActionKind::EnemyInitiatedConflict
                | SituationActionKind::SceneEntersConflict
                | SituationActionKind::Defend
                | SituationActionKind::Counterattack
                | SituationActionKind::TakeCover
                | SituationActionKind::Move
                | SituationActionKind::Hack
                | SituationActionKind::DisableDevice
                | SituationActionKind::Rescue
                | SituationActionKind::UseItem
                | SituationActionKind::InvestigateDuringConflict
        );
    let object = intent.object_interaction_kind.is_some();
    let ability = matches!(
        intent.action_kind,
        SituationActionKind::ActivateAbility | SituationActionKind::TriggerAbility
    ) || intent.target_kind == "ability";
    let advisory_direction_gate = gate.map(is_direction_gate).unwrap_or(false);
    let required_gate = gate
        .map(|g| {
            g.required
                || matches!(
                    g.gate_kind,
                    GateKind::RequiredReactionChoice | GateKind::PlayerRollRequired
                )
        })
        .unwrap_or(false)
        && !advisory_direction_gate;
    let terminal = is_terminal_or_exit_intent(intent);
    let assessment_check =
        intent.target_kind == "assessment_check" || intent.target_kind == "named_check";

    // Advisory direction/stalemate gates never own clear in-frame actions.
    // They are closed/superseded by the reducer and the active frame continues.
    if advisory_direction_gate && frame_continuing && !object {
        return route(
            TurnRouteKind::SupersedeGateThenContinueFrame,
            false,
            true,
            false,
            false,
            false,
            "advisory direction/stalemate gate absorbed by clear semantic frame action",
        );
    }
    if advisory_direction_gate && terminal && active_frame.is_some() {
        return route(
            TurnRouteKind::ContinueFrameFirst,
            false,
            true,
            false,
            false,
            false,
            "terminal/exit intent bypasses advisory direction gate",
        );
    }

    if matches!(
        intent.gate_relation,
        GateRelation::RollReply | GateRelation::ChoiceReply
    ) {
        return route(
            TurnRouteKind::ResolveGateFirst,
            true,
            false,
            false,
            false,
            false,
            "input is a direct hard-gate reply or roll reply",
        );
    }
    // Required reaction/player-roll gates are local blockers. Ordinary attack/object/analysis cannot supersede them.
    // Only terminal/exit/de-escalation table intent may cancel them.
    if has_gate && required_gate && !terminal {
        return route(TurnRouteKind::ResolveGateFirst, true, false, false, false, false, "required gate/reaction must be answered before unrelated object/attack/analysis intent");
    }
    if ability && !required_gate {
        return TurnRoute {
            route_kind: TurnRouteKind::AbilityActivationFirst,
            should_resolve_gate_first: false,
            prefer_conflict_first: false,
            prefer_object_first: false,
            prefer_ability_first: true,
            force_director: false,
            allow_agent_plan: true,
            reason: "semantic ability activation/materialization route".into(),
        };
    }
    if assessment_check {
        return route(TurnRouteKind::StandaloneCheckOrAgent, false, false, false, false, true, "technical/object risk assessment should create an agentic check, not free narration or object patch");
    }
    if has_gate && frame_starting {
        return route(
            TurnRouteKind::SupersedeGateThenStartFrame,
            false,
            true,
            false,
            false,
            false,
            "new frame-worthy combat intent supersedes an old non-required gate",
        );
    }
    // v1.13.1: frame actions have priority over object materialization evidence.
    // A named source object in an attack ("用重型手枪开火") must not be treated as
    // "interact with the pistol". True object actions such as disarm/pickup still
    // reach the object kernel because they are classified as UseItem with object_kind.
    if has_gate && frame_continuing && !object {
        return route(
            TurnRouteKind::SupersedeGateThenContinueFrame,
            false,
            true,
            false,
            false,
            false,
            "frame action supersedes an unrelated non-required gate",
        );
    }
    if has_gate && object {
        return route(
            TurnRouteKind::SupersedeGateThenObjectInteraction,
            false,
            false,
            true,
            false,
            false,
            "object interaction supersedes an unrelated non-required gate",
        );
    }
    if terminal && active_frame.is_some() {
        return route(
            TurnRouteKind::ContinueFrameFirst,
            false,
            true,
            false,
            false,
            false,
            "terminal/exit intent belongs to the active frame reducer",
        );
    }
    // v1.13.1: combat frame routes win over object routes whenever the action is
    // frame-worthy.  Object-first remains correct for disarm/grab/pickup/cut/etc.
    if frame_starting {
        return route(TurnRouteKind::StartFrameFirst, false, true, false, false, false, "frame-worthy action must start a frame before generic checks; named weapons are source objects, not object interactions");
    }
    if frame_continuing && !object {
        return route(TurnRouteKind::ContinueFrameFirst, false, true, false, false, false, "active frame action must route through frame reducer; named weapons stay attached as source objects");
    }
    if object {
        return route(
            TurnRouteKind::ObjectInteractionFirst,
            false,
            false,
            true,
            false,
            false,
            if active_frame.is_some() {
                "object interaction inside an active frame is attached to the frame as a child action"
            } else {
                "object interaction route selected by deterministic route invariant"
            },
        );
    }
    if matches!(intent.action_kind, SituationActionKind::AskQuestion) {
        return route(
            TurnRouteKind::DirectorFirst,
            false,
            false,
            false,
            true,
            true,
            "player is asking for scene guidance or observation",
        );
    }
    route(
        TurnRouteKind::StandaloneCheckOrAgent,
        false,
        false,
        false,
        false,
        true,
        "no frame-worthy or object route detected",
    )
}

fn route(
    kind: TurnRouteKind,
    resolve_gate: bool,
    conflict: bool,
    object: bool,
    director: bool,
    agent: bool,
    reason: &str,
) -> TurnRoute {
    TurnRoute {
        route_kind: kind,
        should_resolve_gate_first: resolve_gate,
        prefer_conflict_first: conflict,
        prefer_object_first: object,
        prefer_ability_first: false,
        force_director: director,
        allow_agent_plan: agent,
        reason: reason.into(),
    }
}

fn semantic_object_interaction_hint(sem: &SemanticIntentResult) -> Option<String> {
    sem.raw_json
        .get("object_interaction_kind")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            let text = serde_json::to_string(&sem.raw_json)
                .unwrap_or_default()
                .to_lowercase();
            if text.contains("disarm") {
                Some("disarm".into())
            } else if text.contains("grab") || text.contains("snatch") {
                Some("grab_held_object".into())
            } else if text.contains("pick_up") || text.contains("pickup") {
                Some("pick_up".into())
            } else if text.contains("cut") && (text.contains("cable") || text.contains("wire")) {
                Some("cut_connection".into())
            } else {
                None
            }
        })
}

fn is_terminal_or_exit_intent(intent: &TurnIntent) -> bool {
    intent
        .evidence_terms
        .iter()
        .any(|e| e == "exit_or_deescalation")
        || matches!(
            intent.frame_relation,
            FrameRelation::ExitAttempt
                | FrameRelation::DeescalationAttempt
                | FrameRelation::Surrender
                | FrameRelation::Flee
                | FrameRelation::HideToDisengage
        )
        || matches!(
            intent.action_kind,
            SituationActionKind::Surrender
                | SituationActionKind::Flee
                | SituationActionKind::Negotiate
                | SituationActionKind::EndConflict
                | SituationActionKind::LeaveScene
        )
}

fn looks_like_roll(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("/roll")
        || t.parse::<i32>().is_ok()
        || t.contains("1d")
        || t.contains("d4")
        || t.contains("d6")
        || t.contains("d8")
        || t.contains("d10")
        || t.contains("d12")
        || t.contains("d20")
        || t.contains("d100")
        || t.contains("掷")
        || t.contains("骰")
        || t.contains("总计")
        || t.contains("合计")
}

fn contains_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

fn semantic_is_available(sem: &SemanticIntentResult) -> bool {
    sem.classifier != "semantic_unavailable_no_route"
}

fn semantic_is_confident(sem: &SemanticIntentResult) -> bool {
    matches!(
        sem.confidence,
        RulingConfidence::High | RulingConfidence::Medium
    )
}

fn semantic_route_bool(sem: &SemanticIntentResult, key: &str) -> bool {
    sem.raw_json
        .get("route_cues")
        .and_then(|v| v.get(key))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn semantic_is_combat_action(sem: &SemanticIntentResult) -> bool {
    semantic_route_bool(sem, "combat_action")
        || matches!(
            sem.primary_action_kind,
            SituationActionKind::Attack
                | SituationActionKind::Counterattack
                | SituationActionKind::Defend
                | SituationActionKind::Dodge
                | SituationActionKind::TakeCover
                | SituationActionKind::Hack
                | SituationActionKind::DisableDevice
                | SituationActionKind::CastOrUsePower
        )
}

fn semantic_is_sustained_combat_action(sem: &SemanticIntentResult) -> bool {
    semantic_route_bool(sem, "sustained_combat_action")
        || matches!(
            sem.primary_action_kind,
            SituationActionKind::Attack | SituationActionKind::Counterattack
        )
}

fn semantic_is_incoming_harm(sem: &SemanticIntentResult) -> bool {
    semantic_route_bool(sem, "incoming_harm")
        || matches!(
            sem.primary_action_kind,
            SituationActionKind::UnderAttack
                | SituationActionKind::EnemyInitiatedConflict
                | SituationActionKind::SceneEntersConflict
        )
}

fn semantic_is_terminal_or_exit(sem: &SemanticIntentResult) -> bool {
    semantic_route_bool(sem, "terminal_or_exit")
        || matches!(
            sem.primary_action_kind,
            SituationActionKind::Flee
                | SituationActionKind::Surrender
                | SituationActionKind::Negotiate
                | SituationActionKind::EndConflict
                | SituationActionKind::LeaveScene
        )
        || matches!(
            sem.frame_relation,
            FrameRelation::ExitAttempt
                | FrameRelation::DeescalationAttempt
                | FrameRelation::Surrender
                | FrameRelation::Flee
                | FrameRelation::HideToDisengage
        )
}

fn semantic_is_object_interaction(sem: &SemanticIntentResult) -> bool {
    semantic_route_bool(sem, "object_interaction")
        || sem
            .raw_json
            .get("object_interaction_kind")
            .and_then(serde_json::Value::as_str)
            .map(|s| !s.is_empty() && s != "none")
            .unwrap_or(false)
}

/// The parameter an EXPLICIT named check tests, taken from the semantic layer's
/// materialization requests: the `actor_parameter` request (the value the roll
/// reads, e.g. "Sanity"/"理智") is preferred, else the `check_contract` request's
/// label. The label is used VERBATIM — the LLM already extracted the parameter
/// name semantically across languages, and the contest resolver canonicalizes it
/// against the kernel by meaning, so no hardcoded keyword/suffix list is needed.
/// Returns None when no such request was emitted.
fn semantic_named_check_parameter(sem: &SemanticIntentResult) -> Option<String> {
    let reqs = sem
        .raw_json
        .get("materialization_requests")
        .and_then(|v| v.as_array())?;
    let label_of = |kind: &str| -> Option<String> {
        reqs.iter()
            .find(|r| r.get("target_kind").and_then(|v| v.as_str()) == Some(kind))
            .and_then(|r| r.get("target_label").and_then(|v| v.as_str()))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    label_of("actor_parameter").or_else(|| label_of("check_contract"))
}

fn semantic_is_assessment_check(sem: &SemanticIntentResult) -> bool {
    semantic_route_bool(sem, "assessment_check")
        || sem
            .raw_json
            .get("primary_action_kind")
            .and_then(serde_json::Value::as_str)
            .map(|s| s == "assess_risk" || s == "analyze_object" || s == "inspect")
            .unwrap_or(false)
}

fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

/// P0-3: lexical keyword matching is an AUDITED FALLBACK, not the normal route.
/// It participates ONLY when the semantic classifier is unavailable or
/// low-confidence. When semantic routing is available AND confident, lexical
/// keywords (including module nouns) must NOT change a routing decision — that
/// is the semantic-over-keyword core principle. The env override
/// (`TRPG_SEMANTIC_COMBAT_LEXICAL_FALLBACK_AUDIT`, default OFF) exists only to
/// force-enable the lexical layer for offline audit/debugging.
fn lexical_fallback_allowed(semantic_available: bool, semantic_confident: bool) -> bool {
    !semantic_available
        || !semantic_confident
        || env_bool("TRPG_SEMANTIC_COMBAT_LEXICAL_FALLBACK_AUDIT", false)
}

fn parse_gate_relation(s: &str) -> Option<GateRelation> {
    match s {
        "no_active_gate" => Some(GateRelation::NoActiveGate),
        "roll_reply" => Some(GateRelation::RollReply),
        "choice_reply" => Some(GateRelation::ChoiceReply),
        "superseding_intent" => Some(GateRelation::SupersedingIntent),
        "unrelated_new_action" => Some(GateRelation::UnrelatedNewAction),
        "ambiguous" => Some(GateRelation::Ambiguous),
        _ => None,
    }
}
fn is_direction_gate(gate: &InteractionGate) -> bool {
    matches!(gate.gate_kind, GateKind::ChooseActionMode)
        || gate
            .advice_refs
            .iter()
            .any(|r| r.contains("direction") || r.contains("stalemate"))
        || gate.prompt_public.contains("改变局面")
        || gate.prompt_public.contains("局势")
}

fn is_actual_object_action(text: &str) -> bool {
    contains_any(
        text,
        &[
            "缴械",
            "打掉武器",
            "夺下武器",
            "抢",
            "夺",
            "抓",
            "捡",
            "拾起",
            "拿起",
            "丢",
            "扔下",
            "放下",
            "装备",
            "穿上",
            "戴上",
            "换弹",
            "装弹",
            "剪线",
            "剪断",
            "切断",
            "撬锁",
            "开锁",
            "搜身",
            "搜尸",
            "翻找",
            "disarm",
            "grab",
            "snatch",
            "steal",
            "pickup",
            "pick up",
            "drop",
            "equip",
            "reload",
            "cut cable",
            "cut wire",
            "unlock",
            "lockpick",
            "loot",
            "frisk",
        ],
    )
}

fn infer_object_interaction_kind(text: &str) -> String {
    if contains_any(text, &["缴械", "disarm"]) {
        "disarm".into()
    } else if contains_any(text, &["抢", "夺", "grab", "steal"]) {
        "grab_held_object".into()
    } else if contains_any(text, &["捡", "拾", "pickup", "pick up"]) {
        "pick_up".into()
    } else if contains_any(text, &["剪", "切断", "cable", "cut"]) {
        "cut_connection".into()
    } else if contains_any(text, &["装备", "穿", "equip"]) {
        "equip".into()
    } else if contains_any(text, &["换弹", "reload"]) {
        "reload".into()
    } else if contains_any(text, &["开锁", "撬锁", "unlock"]) {
        "unlock".into()
    } else {
        "use".into()
    }
}

#[cfg(test)]
mod named_check_tests {
    use super::*;

    fn sem(raw: serde_json::Value) -> SemanticIntentResult {
        SemanticIntentResult {
            semantic_id: "s".into(),
            session_id: "sess".into(),
            turn_id: "t".into(),
            ruleset_id: "call_of_cthulhu_7e".into(),
            primary_action_kind: SituationActionKind::Unknown,
            gate_relation: "no_active_gate".into(),
            frame_relation: FrameRelation::OutsideFrameAction,
            target_refs: vec![],
            object_refs: vec![],
            ability_refs: vec![],
            materialization_requests: vec![],
            secrecy_policy: "gm_only".into(),
            confidence: RulingConfidence::Medium,
            classifier: "test".into(),
            rationale_brief: None,
            raw_json: raw,
            created_at: Utc::now(),
        }
    }
    fn input<'a>() -> TurnOrchestratorInput<'a> {
        TurnOrchestratorInput {
            session_id: "sess",
            turn_id: "t",
            ruleset_id: "call_of_cthulhu_7e",
            module_id: None,
            user_input: "I make a Sanity roll",
        }
    }

    // The named parameter is taken from the semantic actor_parameter request (the
    // tested value), preferred over the check_contract label.
    #[test]
    fn prefers_actor_parameter_label() {
        let s = sem(json!({"materialization_requests":[
            {"target_kind":"check_contract","target_label":"Sanity roll"},
            {"target_kind":"actor_parameter","target_label":"Sanity"}
        ]}));
        assert_eq!(
            semantic_named_check_parameter(&s).as_deref(),
            Some("Sanity")
        );
    }

    // Falls back to the check_contract label verbatim — no keyword post-processing;
    // the resolver canonicalizes it against the kernel by meaning.
    #[test]
    fn falls_back_to_check_contract_label_verbatim() {
        let s = sem(
            json!({"materialization_requests":[{"target_kind":"check_contract","target_label":"Spot Hidden"}]}),
        );
        assert_eq!(
            semantic_named_check_parameter(&s).as_deref(),
            Some("Spot Hidden")
        );
    }

    // The LLM's actor_parameter label is used as-is across languages — the model
    // already extracted the clean parameter name (理智), no hardcoded word list.
    #[test]
    fn uses_cjk_actor_parameter_label_verbatim() {
        let s = sem(
            json!({"materialization_requests":[{"target_kind":"actor_parameter","target_label":"理智"}]}),
        );
        assert_eq!(semantic_named_check_parameter(&s).as_deref(), Some("理智"));
    }

    // No actor_parameter / check_contract request → not a named check.
    #[test]
    fn no_param_request_is_not_named_check() {
        let s = sem(
            json!({"materialization_requests":[{"target_kind":"object_definition","target_label":"Lever"}]}),
        );
        assert_eq!(semantic_named_check_parameter(&s), None);
    }

    // classify_turn_intent routes an explicit named roll to target_kind=named_check
    // and carries the parameter, taking priority over the generic assessment route.
    #[test]
    fn classify_routes_named_check_with_parameter() {
        let s = sem(json!({"materialization_requests":[
            {"target_kind":"check_contract","target_label":"Sanity roll"},
            {"target_kind":"actor_parameter","target_label":"Sanity"}
        ]}));
        let intent = classify_turn_intent(input(), None, None, None, Some(&s));
        assert_eq!(intent.target_kind, "named_check");
        assert_eq!(intent.named_parameter.as_deref(), Some("Sanity"));
    }

    // A combat input that also reads a parameter must NOT be hijacked into a named
    // check — frame/object/exit routes win (护栏: 不抢战斗反应路由). P0-3: the combat
    // signal now comes from the SEMANTIC layer (route_cues.combat_action) rather
    // than the lexical keyword "开枪", since lexical no longer participates when
    // semantic is available+confident.
    #[test]
    fn combat_is_not_hijacked_by_named_check() {
        let s = sem(
            json!({"route_cues":{"combat_action":true},"materialization_requests":[{"target_kind":"actor_parameter","target_label":"Sanity"}]}),
        );
        let inp = TurnOrchestratorInput {
            session_id: "sess",
            turn_id: "t",
            ruleset_id: "call_of_cthulhu_7e",
            module_id: None,
            user_input: "我开枪射击那个怪物",
        };
        let intent = classify_turn_intent(inp, None, None, None, Some(&s));
        assert_ne!(intent.target_kind, "named_check");
    }
}

#[cfg(test)]
mod lexical_fallback_tests {
    use super::*;

    /// Build a semantic result with explicit availability/confidence knobs.
    fn sem(
        classifier: &str,
        confidence: RulingConfidence,
        action: SituationActionKind,
        raw: serde_json::Value,
    ) -> SemanticIntentResult {
        SemanticIntentResult {
            semantic_id: "s".into(),
            session_id: "sess".into(),
            turn_id: "t".into(),
            ruleset_id: "rs".into(),
            primary_action_kind: action,
            gate_relation: "ambiguous".into(),
            frame_relation: FrameRelation::OutsideFrameAction,
            target_refs: vec![],
            object_refs: vec![],
            ability_refs: vec![],
            materialization_requests: vec![],
            secrecy_policy: "gm_only".into(),
            confidence,
            classifier: classifier.into(),
            rationale_brief: None,
            raw_json: raw,
            created_at: Utc::now(),
        }
    }
    fn input<'a>(text: &'a str) -> TurnOrchestratorInput<'a> {
        TurnOrchestratorInput {
            session_id: "sess",
            turn_id: "t",
            ruleset_id: "rs",
            module_id: None,
            user_input: text,
        }
    }

    // ACCEPTANCE (1): with semantic available AND confident, lexical attack
    // keywords ("开枪"/"射击") do NOT change the routing decision. The semantic
    // result carries NO combat signal (primary=Unknown, no route_cues), so the
    // turn stays non-combat despite the keywords. (Pre-flip default this routed
    // to Attack/opposition — that dilution is exactly what P0-3 removes.)
    #[test]
    fn lexical_keywords_do_not_change_routing_when_semantic_confident() {
        let s = sem(
            "llm_semantic_classifier_v1_9",
            RulingConfidence::Medium,
            SituationActionKind::Unknown,
            json!({}),
        );
        let intent = classify_turn_intent(input("我开枪射击那个怪物"), None, None, None, Some(&s));
        assert_ne!(
            intent.action_kind,
            SituationActionKind::Attack,
            "lexical attack keyword must not force Attack when semantic is confident"
        );
        assert_ne!(
            intent.target_kind, "opposition",
            "lexical attack keyword must not force the opposition route"
        );
    }

    // ACCEPTANCE (2) regression: when semantic is UNAVAILABLE, the audited lexical
    // fallback still classifies "我开枪射击" as an attack so deployments without a
    // working semantic classifier keep functioning.
    #[test]
    fn lexical_fallback_still_works_when_semantic_unavailable() {
        let s = sem(
            "semantic_unavailable_no_route",
            RulingConfidence::Low,
            SituationActionKind::Unknown,
            json!({}),
        );
        let intent = classify_turn_intent(input("我开枪射击那个怪物"), None, None, None, Some(&s));
        assert_eq!(
            intent.action_kind,
            SituationActionKind::Attack,
            "lexical fallback must detect attack when semantic is unavailable"
        );
        assert_eq!(intent.target_kind, "opposition");
    }

    // The same input + a confident-but-non-combat semantic result vs. an
    // unavailable one must diverge — proving lexical participation is gated on
    // semantic availability/confidence, not always-on.
    #[test]
    fn routing_diverges_on_semantic_availability_for_same_input() {
        let confident = sem(
            "llm",
            RulingConfidence::Medium,
            SituationActionKind::Unknown,
            json!({}),
        );
        let unavailable = sem(
            "semantic_unavailable_no_route",
            RulingConfidence::Low,
            SituationActionKind::Unknown,
            json!({}),
        );
        let a = classify_turn_intent(
            input("我开枪射击那个怪物"),
            None,
            None,
            None,
            Some(&confident),
        );
        let b = classify_turn_intent(
            input("我开枪射击那个怪物"),
            None,
            None,
            None,
            Some(&unavailable),
        );
        assert_ne!(
            a.action_kind, b.action_kind,
            "lexical keyword changes routing ONLY when semantic is unavailable"
        );
    }

    // The gate predicate itself: default OFF when available+confident; ON when
    // unavailable or low-confidence.
    #[test]
    fn lexical_fallback_allowed_defaults_off_only_when_confident() {
        assert!(
            !lexical_fallback_allowed(true, true),
            "available+confident -> lexical OFF by default"
        );
        assert!(
            lexical_fallback_allowed(false, true),
            "unavailable -> lexical ON"
        );
        assert!(
            lexical_fallback_allowed(true, false),
            "low-confidence -> lexical ON"
        );
        assert!(
            lexical_fallback_allowed(false, false),
            "unavailable+low -> lexical ON"
        );
    }
}
