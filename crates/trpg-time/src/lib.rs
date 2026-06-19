use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use trpg_db::Db;
use trpg_model::*;
use uuid::Uuid;

#[derive(Clone)]
pub struct WorldTimeService {
    pub db: Db,
}

impl WorldTimeService {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn ensure_session_time(
        &self,
        session_id: &str,
        campaign_id: Option<&str>,
    ) -> Result<WorldTimeState> {
        if let Some(state) = self.db.get_world_time_state(session_id).await? {
            return Ok(state);
        }
        let state = default_world_time_state(session_id, campaign_id.unwrap_or(session_id));
        self.db.upsert_world_time_state(&state).await?;
        let event = WorldEvent {
            event_id: format!("world_event_{}", Uuid::new_v4().simple()),
            campaign_id: state.campaign_id.clone(),
            session_id: state.session_id.clone(),
            world_tick: state.world_tick,
            event_seq: 1,
            turn_id: None,
            frame_id: None,
            event_kind: WorldEventKind::SystemEvent,
            event_json: json!({"kind":"world_time_initialized", "display_time": state.display_time}),
            visibility: Visibility::SystemOnly,
            source_refs: vec![],
            caused_by_event_ids: vec![],
            state_patch_ids: vec![],
            created_at: Utc::now(),
        };
        self.db.insert_world_event(&event).await.ok();
        Ok(state)
    }

    pub async fn current(&self, session_id: &str) -> Result<WorldTimeState> {
        self.ensure_session_time(session_id, None).await
    }

    pub async fn record_event(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        frame_id: Option<&str>,
        kind: WorldEventKind,
        event_json: serde_json::Value,
        visibility: Visibility,
    ) -> Result<WorldEvent> {
        let mut state = self.ensure_session_time(session_id, None).await?;
        state.event_seq += 1;
        state.updated_at = Utc::now();
        self.db.upsert_world_time_state(&state).await?;
        let event = WorldEvent {
            event_id: format!("world_event_{}", Uuid::new_v4().simple()),
            campaign_id: state.campaign_id.clone(),
            session_id: state.session_id.clone(),
            world_tick: state.world_tick,
            event_seq: state.event_seq,
            turn_id: turn_id.map(str::to_string),
            frame_id: frame_id.map(str::to_string),
            event_kind: kind,
            event_json,
            visibility,
            source_refs: vec![],
            caused_by_event_ids: vec![],
            state_patch_ids: vec![],
            created_at: Utc::now(),
        };
        self.db.insert_world_event(&event).await?;
        Ok(event)
    }

    pub async fn advance(&self, req: TimeAdvanceRequest) -> Result<TimeAdvanceResult> {
        let from = self
            .ensure_session_time(&req.session_id, req.campaign_id.as_deref())
            .await?;
        if matches!(
            req.mutation_kind,
            TimeMutationKind::NoAdvance | TimeMutationKind::FlashbackFrame
        ) {
            let event = self.record_event(
                &req.session_id,
                req.caused_by_turn_id.as_deref(),
                None,
                WorldEventKind::TimeAdvanced,
                json!({"mutation_kind": req.mutation_kind.as_str(), "reason": req.reason, "advance_seconds": 0}),
                req.visibility,
            ).await?;
            return Ok(TimeAdvanceResult {
                from: from.clone(),
                to: from,
                advance_event: Some(event),
                triggered_events: vec![],
                scheduled_events_due: vec![],
                expired_effect_ids: vec![],
            });
        }
        let delta_seconds = req.amount.total_seconds().max(0);
        let delta_ticks = delta_seconds.max(1);
        let mut to = from.clone();
        to.world_tick += delta_ticks;
        to.absolute_seconds += delta_seconds;
        to.time_scale = req.scale;
        if let Some(epoch) = req.scene_epoch.clone() {
            to.scene_epoch = Some(epoch);
        }
        to.event_seq += 1;
        to.updated_at = Utc::now();
        to.display_time = format_display_time(to.absolute_seconds, to.time_scale);
        self.db.upsert_world_time_state(&to).await?;

        let advance_event = WorldEvent {
            event_id: format!("world_event_{}", Uuid::new_v4().simple()),
            campaign_id: to.campaign_id.clone(),
            session_id: to.session_id.clone(),
            world_tick: to.world_tick,
            event_seq: to.event_seq,
            turn_id: req.caused_by_turn_id.clone(),
            frame_id: None,
            event_kind: WorldEventKind::TimeAdvanced,
            event_json: json!({
                "from_tick": from.world_tick,
                "to_tick": to.world_tick,
                "from_display": from.display_time,
                "to_display": to.display_time,
                "amount": req.amount,
                "scale": req.scale,
                "reason": req.reason,
                "mutation_kind": req.mutation_kind,
                "caused_by_event_id": req.caused_by_event_id,
            }),
            visibility: req.visibility,
            source_refs: vec![],
            caused_by_event_ids: req.caused_by_event_id.iter().cloned().collect(),
            state_patch_ids: vec![],
            created_at: Utc::now(),
        };
        self.db.insert_world_event(&advance_event).await?;
        self.db
            .insert_world_time_advance(&from, &to, &advance_event)
            .await
            .ok();

        let due = self
            .db
            .list_due_scheduled_events(&to.session_id, to.world_tick)
            .await?;
        let mut triggered_events = Vec::new();
        for scheduled in &due {
            let ev = WorldEvent {
                event_id: format!("world_event_{}", Uuid::new_v4().simple()),
                campaign_id: to.campaign_id.clone(),
                session_id: to.session_id.clone(),
                world_tick: to.world_tick,
                event_seq: to.event_seq + triggered_events.len() as i64 + 1,
                turn_id: req.caused_by_turn_id.clone(),
                frame_id: None,
                event_kind: WorldEventKind::ScheduledEventDue,
                event_json: json!({"scheduled_event": scheduled}),
                visibility: scheduled.visibility,
                source_refs: vec![],
                caused_by_event_ids: scheduled.created_by_event_id.iter().cloned().collect(),
                state_patch_ids: vec![],
                created_at: Utc::now(),
            };
            self.db.insert_world_event(&ev).await.ok();
            self.db
                .update_scheduled_event_status(
                    &scheduled.scheduled_event_id,
                    ScheduledEventStatus::Due,
                )
                .await
                .ok();
            triggered_events.push(ev);
        }
        if !triggered_events.is_empty() {
            let mut final_state = to.clone();
            final_state.event_seq += triggered_events.len() as i64;
            final_state.updated_at = Utc::now();
            self.db.upsert_world_time_state(&final_state).await.ok();
        }
        Ok(TimeAdvanceResult {
            from,
            to,
            advance_event: Some(advance_event),
            triggered_events,
            scheduled_events_due: due,
            expired_effect_ids: vec![],
        })
    }

    pub async fn schedule_in(
        &self,
        session_id: &str,
        amount: TimeAmount,
        event_kind: WorldEventKind,
        payload_json: serde_json::Value,
        visibility: Visibility,
        created_by_event_id: Option<String>,
    ) -> Result<ScheduledEvent> {
        let state = self.ensure_session_time(session_id, None).await?;
        let due_tick = state.world_tick + amount.total_seconds().max(1);
        let scheduled = ScheduledEvent {
            scheduled_event_id: format!("scheduled_{}", Uuid::new_v4().simple()),
            campaign_id: state.campaign_id.clone(),
            session_id: session_id.to_string(),
            due_tick,
            event_kind,
            payload_json,
            visibility,
            status: ScheduledEventStatus::Pending,
            created_by_event_id,
            created_at: Utc::now(),
        };
        self.db.insert_scheduled_event(&scheduled).await?;
        Ok(scheduled)
    }
}

pub fn default_world_time_state(session_id: &str, campaign_id: &str) -> WorldTimeState {
    WorldTimeState {
        campaign_id: campaign_id.to_string(),
        session_id: session_id.to_string(),
        world_tick: 0,
        absolute_seconds: 0,
        calendar_id: std::env::var("TRPG_WORLD_TIME_CALENDAR_ID")
            .unwrap_or_else(|_| "relative_default".into()),
        display_time: std::env::var("TRPG_WORLD_TIME_START_DISPLAY")
            .unwrap_or_else(|_| "Day 1, 00:00".into()),
        time_scale: TimeScale::SceneBeat,
        scene_epoch: Some("session_start".into()),
        turn_seq: 0,
        event_seq: 0,
        updated_at: Utc::now(),
    }
}

pub fn format_display_time(seconds: i64, scale: TimeScale) -> String {
    let days = seconds.div_euclid(86_400) + 1;
    let rem = seconds.rem_euclid(86_400);
    let hours = rem / 3600;
    let minutes = (rem % 3600) / 60;
    match scale {
        TimeScale::CombatRound => format!("Day {days}, {hours:02}:{minutes:02} · combat time"),
        TimeScale::Travel => format!("Day {days}, {hours:02}:{minutes:02} · travel"),
        TimeScale::Downtime => format!("Day {days}, {hours:02}:{minutes:02} · downtime"),
        TimeScale::Flashback => format!("Day {days}, {hours:02}:{minutes:02} · flashback"),
        _ => format!("Day {days}, {hours:02}:{minutes:02}"),
    }
}
