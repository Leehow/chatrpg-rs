use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;
use trpg_time::WorldTimeService;
use uuid::Uuid;

#[derive(Clone)]
pub struct InteractionLifecycleKernel {
    pub db: Db,
}

impl InteractionLifecycleKernel {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn enabled() -> bool {
        std::env::var("TRPG_INTERACTION_KERNEL_ENABLE_V15")
            .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
            .unwrap_or(true)
    }

    pub async fn open_context_for_frame(
        &self,
        frame: &StateFrame,
        _kind: InteractionContextKind,
    ) -> Result<InteractionContext> {
        self.ensure_frame_context(frame).await
    }

    pub async fn attach_gate_to_active_frame(
        &self,
        session_id: &str,
        gate_id: &str,
        frame_id: Option<&str>,
    ) -> Result<()> {
        if !Self::enabled() {
            return Ok(());
        }
        if let Some(frame_id) = frame_id {
            self.attach_gate_to_frame(session_id, gate_id, frame_id)
                .await?;
        }
        Ok(())
    }

    pub async fn close_frame_cascade(
        &self,
        frame_id: &str,
        session_id: &str,
        reason: SupersededReason,
        detail: &str,
    ) -> Result<InteractionTransitionResult> {
        self.close_frame(
            session_id,
            frame_id,
            format!("{}:{}", reason.as_str(), detail),
        )
        .await
    }

    pub async fn reconcile_session(&self, session_id: &str) -> Result<InteractionTransitionResult> {
        if !Self::enabled() {
            return Ok(InteractionTransitionResult::default());
        }
        let time = WorldTimeService::new(self.db.clone())
            .current(session_id)
            .await
            .unwrap_or_default();
        let generation = self
            .db
            .ensure_interaction_generation(session_id)
            .await
            .unwrap_or(0);
        let repairs = self
            .db
            .reconcile_interaction_lifecycle(session_id, time.world_tick, generation)
            .await
            .unwrap_or_default();
        let mut result = InteractionTransitionResult::default();
        result.repairs = repairs.clone();
        for repair in &repairs {
            let _ = self.db.insert_invariant_repair(session_id, repair).await;
        }
        if !repairs.is_empty() {
            if let Ok(event) = self
                .write_interaction_event(
                    session_id,
                    None,
                    None,
                    time.world_tick,
                    generation,
                    InteractionEventKind::ReconcileSession,
                    json!({"repairs": repairs}),
                )
                .await
            {
                result.events.push(event);
            }
        }
        Ok(result)
    }

    pub async fn ensure_frame_context(&self, frame: &StateFrame) -> Result<InteractionContext> {
        if !Self::enabled() {
            return Ok(InteractionContext::default());
        }
        let time = WorldTimeService::new(self.db.clone())
            .current(&frame.session_id)
            .await
            .unwrap_or_default();
        let generation = self
            .db
            .ensure_interaction_generation(&frame.session_id)
            .await
            .unwrap_or(0);
        let context_id = format!("ctx_{}", frame.frame_id);
        let ctx = InteractionContext {
            context_id: context_id.clone(),
            session_id: frame.session_id.clone(),
            frame_id: Some(frame.frame_id.clone()),
            context_kind: InteractionContextKind::Frame,
            status: match frame.status {
                FrameStatus::Active | FrameStatus::Resolving | FrameStatus::Paused => {
                    InteractionContextStatus::Active
                }
                _ => InteractionContextStatus::Closed,
            },
            generation,
            active_gate_ids: frame.active_gate_ids.clone(),
            pending_check_ids: vec![],
            reaction_window_ids: vec![],
            object_interaction_ids: vec![],
            child_context_ids: vec![],
            opened_at_tick: time.world_tick,
            closed_at_tick: None,
            owner: InteractionOwner {
                owner_kind: "state_frame".into(),
                owner_id: Some(frame.frame_id.clone()),
                owner_json: json!({"frame_kind": frame.frame_kind.as_str(), "title": frame.title}),
            },
            context_json: json!({"frame_status": frame.status.as_str(), "objective": frame.objective}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.db.upsert_interaction_context(&ctx).await.ok();
        Ok(ctx)
    }

    pub async fn attach_gate_to_frame(
        &self,
        session_id: &str,
        gate_id: &str,
        frame_id: &str,
    ) -> Result<()> {
        if !Self::enabled() {
            return Ok(());
        }
        let generation = self
            .db
            .ensure_interaction_generation(session_id)
            .await
            .unwrap_or(0);
        self.db
            .attach_gate_to_frame(gate_id, frame_id, generation)
            .await?;
        Ok(())
    }

    pub async fn attach_pending_check_to_frame(
        &self,
        session_id: &str,
        check_id: &str,
        frame_id: &str,
        gate_id: Option<&str>,
    ) -> Result<()> {
        if !Self::enabled() {
            return Ok(());
        }
        let generation = self
            .db
            .ensure_interaction_generation(session_id)
            .await
            .unwrap_or(0);
        self.db
            .attach_pending_check_to_frame(check_id, frame_id, gate_id, generation)
            .await?;
        Ok(())
    }

    pub async fn attach_object_interaction_to_active_frame(
        &self,
        session_id: &str,
        interaction_id: &str,
        frame_id: &str,
    ) -> Result<()> {
        if !Self::enabled() {
            return Ok(());
        }
        let generation = self
            .db
            .ensure_interaction_generation(session_id)
            .await
            .unwrap_or(0);
        sqlx::query("update object_interaction_contracts set frame_id = $2, interaction_context_id = coalesce(interaction_context_id, 'ctx_' || $2), updated_at = now() where session_id = $1 and interaction_id = $3")
            .bind(session_id)
            .bind(frame_id)
            .bind(interaction_id)
            .execute(&self.db.pool)
            .await
            .ok();
        sqlx::query("update interaction_contexts set object_interaction_ids = array_append(object_interaction_ids, $3), updated_at = now() where session_id = $1 and frame_id = $2 and status = 'active'")
            .bind(session_id)
            .bind(frame_id)
            .bind(interaction_id)
            .execute(&self.db.pool)
            .await
            .ok();
        let time = WorldTimeService::new(self.db.clone())
            .current(session_id)
            .await
            .unwrap_or_default();
        let _ = self
            .write_interaction_event(
                session_id,
                Some(format!("ctx_{frame_id}")),
                Some(frame_id.to_string()),
                time.world_tick,
                generation,
                InteractionEventKind::CreateObjectInteraction,
                json!({"interaction_id": interaction_id}),
            )
            .await;
        Ok(())
    }

    pub async fn close_frame(
        &self,
        session_id: &str,
        frame_id: &str,
        reason: impl Into<String>,
    ) -> Result<InteractionTransitionResult> {
        if !Self::enabled() {
            return Ok(InteractionTransitionResult::default());
        }
        let reason = reason.into();
        let time = WorldTimeService::new(self.db.clone())
            .current(session_id)
            .await
            .unwrap_or_default();
        let generation = self
            .db
            .ensure_interaction_generation(session_id)
            .await
            .unwrap_or(0);
        let repairs = self
            .db
            .cascade_close_frame_interactions(
                session_id,
                frame_id,
                &reason,
                time.world_tick,
                generation,
            )
            .await
            .unwrap_or_default();
        let new_generation = self
            .db
            .bump_interaction_generation(session_id)
            .await
            .unwrap_or(generation + 1);
        let mut result = InteractionTransitionResult::default();
        result.repairs = repairs.clone();
        for repair in &repairs {
            let _ = self.db.insert_invariant_repair(session_id, repair).await;
        }
        if let Ok(event) = self
            .write_interaction_event(
                session_id,
                Some(format!("ctx_{frame_id}")),
                Some(frame_id.to_string()),
                time.world_tick,
                new_generation,
                InteractionEventKind::CloseFrameCascade,
                json!({"reason": reason, "frame_id": frame_id, "repairs": repairs}),
            )
            .await
        {
            result.events.push(event);
        }
        let _ = WorldTimeService::new(self.db.clone()).record_event(session_id, None, Some(frame_id), WorldEventKind::FrameClosed, json!({"reason":"interaction_lifecycle_close_frame", "frame_id": frame_id, "generation": new_generation}), Visibility::GmOnly).await;
        Ok(result)
    }

    pub async fn supersede_gate_for_terminal_intent(
        &self,
        gate: &InteractionGate,
        reason: &str,
        user_input: &str,
    ) -> Result<InteractionTransitionResult> {
        if !Self::enabled() {
            return Ok(InteractionTransitionResult::default());
        }
        let time = WorldTimeService::new(self.db.clone())
            .current(&gate.session_id)
            .await
            .unwrap_or_default();
        let generation = self
            .db
            .ensure_interaction_generation(&gate.session_id)
            .await
            .unwrap_or(0);
        self.db
            .supersede_interaction_gate(&gate.gate_id, reason, time.world_tick)
            .await
            .ok();
        let mut result = InteractionTransitionResult::default();
        if let Ok(event) = self
            .write_interaction_event(
                &gate.session_id,
                gate.interaction_context_id.clone(),
                gate.owner_frame_id.clone(),
                time.world_tick,
                generation,
                InteractionEventKind::SupersedeGate,
                json!({"gate_id": gate.gate_id, "reason": reason, "user_input": user_input}),
            )
            .await
        {
            result.events.push(event);
        }
        Ok(result)
    }

    pub fn player_input_supersedes_gate(&self, gate: &InteractionGate, user_input: &str) -> bool {
        let s = user_input.to_lowercase();
        let terminal_terms = [
            "战斗结束",
            "停火",
            "全员停火",
            "结束战斗",
            "投降",
            "放下武器",
            "和解",
            "讲和",
            "撤退",
            "离开这里",
            "离开这片区域",
            "end combat",
            "ceasefire",
            "stand down",
            "surrender",
            "retreat",
            "leave the scene",
            "negotiate truce",
        ];
        if terminal_terms.iter().any(|t| s.contains(t)) {
            return true;
        }
        matches!(
            gate.on_new_action,
            GateFallbackPolicy::CancelGateAndContinue
        ) && looks_like_new_action_or_cancel(&s)
    }

    async fn write_interaction_event(
        &self,
        session_id: &str,
        context_id: Option<String>,
        frame_id: Option<String>,
        world_tick: i64,
        generation: i64,
        kind: InteractionEventKind,
        event_json: serde_json::Value,
    ) -> Result<InteractionEvent> {
        let event = InteractionEvent {
            event_id: format!("interaction_event_{}", Uuid::new_v4().simple()),
            session_id: session_id.to_string(),
            interaction_context_id: context_id,
            frame_id,
            world_tick,
            generation,
            event_kind: kind,
            event_json,
            created_at: Utc::now(),
        };
        self.db.insert_interaction_event(&event).await.ok();
        Ok(event)
    }
}

fn looks_like_new_action_or_cancel(s: &str) -> bool {
    [
        "算了",
        "改做",
        "不做",
        "放弃",
        "换",
        "instead",
        "rather",
        "never mind",
        "cancel",
    ]
    .iter()
    .any(|t| s.contains(t))
}
