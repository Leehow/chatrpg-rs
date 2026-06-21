//! Response Contract (蓝图 §四): for each player action, the structured set of
//! information FIELDS it requests, plus whether the GM reply addressed each.
//!
//! This upgrades RESPONSE_INTENT_MISMATCH from a binary turn flag to field-level:
//! a single action may ask for several distinct things (e.g. 「逼问头目：是谁…」 =
//! Extract + Identity), and the evaluator reports *which requested field* the GM
//! left unanswered. The "answered" signal stays metric-driven — a field counts as
//! unanswered only when the GM reply carries a no-info marker (no new fact for the
//! asked field), the same shared signal the v1 binary probe used, so the
//! milestone-1 fixtures remain a superset (M1 stays green) and the clean control
//! still passes.

use crate::model::{EvalFinding, RootCause, Severity, Transcript, Turn};
use crate::probes::{contains_any, NO_INFO_MARKERS};
use serde::Serialize;

/// A semantic category of information a player action can request (蓝图 §四 intent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum IntentField {
    /// Who someone is / identity ("是谁", "身份").
    Identity,
    /// Why / motive / cause ("为什么", "为何", "动机", "原因").
    Reason,
    /// How many / counting ("数清", "几个", "多少", "几名").
    Count,
    /// Judgement / appraisal ("评估", "判断", "分辨", "胜算").
    Assess,
    /// Verify / confirm a known fact ("看清", "核对", "确认").
    Perceive,
    /// Extract info from an unwilling NPC ("逼问", "盘问", "质问", "交代").
    Extract,
    /// General inquiry ("打听", "询问", "问清", "问他", "问").
    Inquire,
}

impl IntentField {
    pub const ALL: [IntentField; 7] = [
        IntentField::Identity,
        IntentField::Reason,
        IntentField::Count,
        IntentField::Assess,
        IntentField::Perceive,
        IntentField::Extract,
        IntentField::Inquire,
    ];

    /// Stable screaming-snake id used in evidence + JSON.
    pub fn id(&self) -> &'static str {
        match self {
            IntentField::Identity => "IDENTITY",
            IntentField::Reason => "REASON",
            IntentField::Count => "COUNT",
            IntentField::Assess => "ASSESS",
            IntentField::Perceive => "PERCEIVE",
            IntentField::Extract => "EXTRACT",
            IntentField::Inquire => "INQUIRE",
        }
    }

    /// Markers that signal this field was requested. Order matters only for the
    /// evidence quote; a field fires if ANY marker is present in the action.
    fn markers(&self) -> &'static [&'static str] {
        match self {
            IntentField::Identity => &["是谁", "身份", "是哪", "谁是", "受谁", "是谁指使"],
            IntentField::Reason => &["为什么", "为何", "动机", "原因", "为啥"],
            IntentField::Count => &["数清", "几个", "多少", "几名", "几人", "人数"],
            IntentField::Assess => &["评估", "判断", "分辨", "胜算", "估量"],
            IntentField::Perceive => &["看清", "核对", "确认", "辨认"],
            IntentField::Extract => &["逼问", "盘问", "质问", "交代", "套问", "审问"],
            // No bare "问": it is a substring of 逼问/质问/盘问 (Extract). Real
            // inquiries are caught by the explicit compounds below.
            IntentField::Inquire => &["打听", "询问", "问清", "问他", "问出", "打探"],
        }
    }
}

/// One requested information field within a turn's contract, with whether the GM
/// reply addressed it.
#[derive(Debug, Clone, Serialize)]
pub struct FieldRequest {
    pub field: IntentField,
    /// The marker token matched in the player action (evidence).
    pub marker: String,
    pub answered: bool,
}

/// The Response Contract for a single turn (蓝图 §四).
#[derive(Debug, Clone, Serialize)]
pub struct ResponseContract {
    pub turn: u32,
    pub requests: Vec<FieldRequest>,
}

impl ResponseContract {
    /// Requested fields the GM reply did not address.
    pub fn unanswered(&self) -> impl Iterator<Item = &FieldRequest> {
        self.requests.iter().filter(|r| !r.answered)
    }
}

/// Build the contract for one turn. A field is `answered = false` only when the
/// GM reply carries a no-info marker — keeping the verdict metric-driven.
pub fn contract_for(turn: &Turn) -> ResponseContract {
    let reply = turn.visible_text();
    let no_info = contains_any(&reply, NO_INFO_MARKERS).is_some();
    let mut requests: Vec<FieldRequest> = Vec::new();
    for field in IntentField::ALL {
        if let Some(marker) = contains_any(&turn.player_action, field.markers()) {
            // De-dupe: a coarse field (e.g. Inquire「问」) should not double-count
            // a turn already covered by a sharper one (Extract「逼问」).
            if requests.iter().any(|r| r.field == field) {
                continue;
            }
            requests.push(FieldRequest {
                field,
                marker,
                answered: !no_info,
            });
        }
    }
    ResponseContract {
        turn: turn.index,
        requests,
    }
}

/// All turns that carry at least one requested field.
pub fn contracts(t: &Transcript) -> Vec<ResponseContract> {
    t.turns
        .iter()
        .map(contract_for)
        .filter(|c| !c.requests.is_empty())
        .collect()
}

/// Field-level RESPONSE_INTENT_MISMATCH (蓝图 §四 逐字段查 GM 答没答). Reports the
/// exact information fields the player requested that the GM left unanswered.
pub fn response_intent_mismatch(t: &Transcript) -> Vec<EvalFinding> {
    let mut ids: Vec<u32> = Vec::new();
    let mut evidence: Vec<String> = Vec::new();
    let mut field_total = 0usize;
    for c in contracts(t) {
        let unanswered: Vec<&FieldRequest> = c.unanswered().collect();
        if unanswered.is_empty() {
            continue;
        }
        ids.push(c.turn);
        field_total += unanswered.len();
        if evidence.len() < 4 {
            let fields = unanswered
                .iter()
                .map(|r| format!("{}「{}」", r.field.id(), r.marker))
                .collect::<Vec<_>>()
                .join(" + ");
            evidence.push(format!("turn {} 请求 [{}] 未获答复", c.turn, fields));
        }
    }
    if ids.is_empty() {
        return vec![];
    }
    let turn_count = ids.len();
    vec![EvalFinding::new(
        RootCause::ResponseIntentMismatch,
        Severity::Hard,
        ids,
        "GM 应逐字段回应玩家请求的每一项信息(回答所问/要求检定/说明为何无法获知)",
        format!("{turn_count} 个回合共 {field_total} 个被请求的信息字段未获答复"),
        evidence,
    )]
}
