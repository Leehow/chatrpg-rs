//! Phase-2 shared mechanics types (single home — never added to lib.rs):
//! rule-kernel mechanics catalog (§1), scene mechanic intents (§3), mechanic
//! dues (§4) and the pure helpers `expressiveness_tier`/`granularity_seconds`/
//! `track_semantic_line`. Producers live elsewhere (trpg-rule-agent compile
//! pass, trpg-mechanics watcher, trpg-gm obligations); this module holds only
//! data shapes and pure functions, all backward-compatible via serde defaults.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SourceRef;

// ===== §1 Mechanics catalog (new kernel region) =====

/// One entry of the kernel mechanics catalog. `when_to_use` is the *only*
/// routing basis (semantic, never a keyword table). `source_refs` empty is the
/// single fail-closed drop condition applied by the finalize guard.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MechanicEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: MechanicKind,
    /// What this mechanic *is* (semantic knowledge for the agent).
    #[serde(default)]
    pub description: String,
    /// Trigger semantics — the sole basis for semantic routing.
    #[serde(default)]
    pub when_to_use: String,
    /// Sheet-schema key; the finalize guard checks it exists.
    #[serde(default)]
    pub tested_parameter: Option<String>,
    /// May be empty (semantic tier).
    #[serde(default)]
    pub procedure: Vec<ProcedureStep>,
    #[serde(default)]
    pub hooks: Vec<EngineHook>,
    /// Passive-modifier tier: always-on projection template,
    /// e.g. "信用评级 {value}：{band}".
    #[serde(default)]
    pub passive_projection: Option<String>,
    #[serde(default)]
    pub followup_links: Vec<FollowupLink>,
    /// Conditional-unlock semantics (purely semantic field).
    #[serde(default)]
    pub locked_until: Option<String>,
    /// Empty -> finalize drops the entry (fail-closed = "never invent").
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

/// Expression-shape classification, *not* a discovery filter: anything that
/// does not fit a known kind is still collected, as `Other`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MechanicKind {
    SkillCheck,
    SubsystemProcedure,
    Reaction,
    Spend,
    #[serde(untagged)]
    Other(String),
}

impl Default for MechanicKind {
    /// Contract note: the default lands on `Other(String::new())` so a missing
    /// `kind` key deserializes to it via `#[serde(default)]`.
    fn default() -> Self {
        Self::Other(String::new())
    }
}

/// Procedure step (expressiveness tier 1). Discriminated by the `step` tag;
/// any step that cannot be structured is downgraded whole into `Other`,
/// preserving the original JSON (guardrail §3.5: downgrade, never drop).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum ProcedureStep {
    Roll {
        dice: String,
        #[serde(default)]
        vs: Option<String>,
        #[serde(default)]
        note: Option<String>,
    },
    Apply {
        track: String,
        op: String,
        amount: String,
        #[serde(default)]
        note: Option<String>,
    },
    TableRoll {
        table_ref: String,
        #[serde(default)]
        note: Option<String>,
    },
    Gate {
        condition: String,
        #[serde(default)]
        note: Option<String>,
    },
    #[serde(untagged)]
    Other(serde_json::Value),
}

/// Engine hook (expressiveness tier 2; the third trigger path). This enum IS
/// the engine-event vocabulary: parsing a hook the engine has no event point
/// for must fail (hard serde Err) so the finalize guard can downgrade the
/// entry to semantic and record it in the validation report.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EngineHook {
    SceneEnter,
    TurnStart,
    TimeAdvance,
    Rest,
    CombatStart,
    CombatEnd,
    Calendar { granularity: CalendarGranularity },
    SessionEnd,
    DevelopmentPhase,
}

/// Calendar granularity as data (Fate's 8-segment day proved the granularity
/// is ruleset-defined, not a fixed enum).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CalendarGranularity {
    /// "day" | "week" | "month" | "segment" | custom.
    pub unit: String,
    /// Custom period in seconds (takes precedence when present).
    #[serde(default)]
    pub seconds_per_unit: Option<i64>,
    /// For unit=="segment": 86400 / segments.
    #[serde(default)]
    pub segments_per_day: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
}

/// Pure: granularity -> seconds; unrecognized -> None (fail-closed: the hook
/// degrades to semantic, never guess). `seconds_per_unit` wins when present;
/// "month" is irregular, so the data must provide `seconds_per_unit`.
pub fn granularity_seconds(g: &CalendarGranularity) -> Option<i64> {
    if let Some(s) = g.seconds_per_unit {
        return if s > 0 { Some(s) } else { None };
    }
    match g.unit.as_str() {
        "day" => Some(86_400),
        "week" => Some(604_800),
        "segment" => match g.segments_per_day {
            Some(n) if n > 0 => Some(86_400 / i64::from(n)),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FollowupLink {
    pub condition: FollowupCondition,
    /// Finalize guard: must point at a real catalog entry id, otherwise the
    /// link is dropped and recorded in the validation report.
    pub procedure_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FollowupCondition {
    Threshold { track_id: String, threshold_ref: String },
    OutcomeBand { band_id: String },
    Outcome { value: String },
    /// Downgrade-and-keep (original JSON preserved).
    #[serde(untagged)]
    Other(serde_json::Value),
}

/// Expressiveness tiers, computed from data features on demand — no redundant
/// stored field (guardrail §3.5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressivenessTier {
    Procedure,
    Hook,
    PassiveModifier,
    Semantic,
}

/// Pure tier judgement: non-empty procedure -> Procedure; else non-empty
/// hooks -> Hook; else passive_projection present -> PassiveModifier;
/// otherwise Semantic.
pub fn expressiveness_tier(e: &MechanicEntry) -> ExpressivenessTier {
    if !e.procedure.is_empty() {
        ExpressivenessTier::Procedure
    } else if !e.hooks.is_empty() {
        ExpressivenessTier::Hook
    } else if e.passive_projection.is_some() {
        ExpressivenessTier::PassiveModifier
    } else {
        ExpressivenessTier::Semantic
    }
}

// ===== §3 Scene mechanic intents (module side) =====

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SceneMechanicIntent {
    pub intent_id: String,
    /// Which player action triggers this (semantic match; structural
    /// references go by intent_id — guardrail §3.5.2).
    pub description: String,
    pub tested_parameter: String,
    /// {kind:"dv",value:13} / CoC difficulty band — kept open as a Value.
    #[serde(default)]
    pub difficulty: Option<serde_json::Value>,
    #[serde(default)]
    pub effect_policy: EffectPolicy,
    /// Empty -> apply_deep_to_node drops the intent (fail-closed, no invention).
    pub source_anchor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct EffectPolicy {
    #[serde(default)]
    pub on_success: Vec<EffectPatchIntent>,
    #[serde(default)]
    pub on_failure: Vec<EffectPatchIntent>,
}

/// Maps onto existing primitives only (no new effect machinery):
/// object state -> StatePatch::ObjectPatch, track math -> apply_direct_effect,
/// facts -> StatePatch::CreateFact, countdowns -> WorldTimeService::schedule_in.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPatchIntent {
    SetObjectState {
        object_id: String,
        patch: serde_json::Value,
    },
    ModifyTrack {
        owner_kind: String,
        owner_id: Option<String>,
        track_id: String,
        op: String,
        amount: i64,
    },
    CreateFact {
        target: String,
        fact: serde_json::Value,
    },
    StartCountdown {
        label: String,
        amount: i64,
        scale: String,
        payload: serde_json::Value,
    },
    /// Fail-closed: never executed, never aborts — the executor records an
    /// "unexecutable_intent" fact instead so it stays observable.
    #[serde(untagged)]
    Other(serde_json::Value),
}

// ===== §4 MechanicDue (produced by trpg-mechanics watcher, consumed by
// trpg-gm obligations) =====

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MechanicDue {
    /// "due_{uuid simple}".
    pub due_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub source: DueSource,
    /// Threshold dues: the kernel track id.
    #[serde(default)]
    pub source_track: Option<String>,
    /// Hook dues: "scene_enter"/"calendar"/… (EngineHook serde tag value).
    #[serde(default)]
    pub hook_event: Option<String>,
    /// Hook dues: the catalog entry id this hook hangs off (structured
    /// binding, guardrail §3.5.1).
    #[serde(default)]
    pub mechanic_id: Option<String>,
    /// Prose consequence — without a followup_procedure_id the agent handles
    /// the due from this semantics (fail-closed but never silent).
    pub threshold_desc: String,
    #[serde(default)]
    pub followup_procedure_id: Option<String>,
    /// "actor" | "scene" | "party" | "world" (open vocabulary).
    pub owner_kind: String,
    pub owner_id: String,
    /// Threshold: {"before":38,"after":32,"delta":-6};
    /// hook: {"event":"scene_enter","scene_id":"sc02"}.
    pub evidence: serde_json::Value,
    #[serde(default)]
    pub status: DueStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DueSource {
    Threshold,
    Hook,
    /// 刺激驱动检定预 pass：目录 when_to_use 与本回合虚构内容语义命中
    /// （恐怖刺激→SAN 检定一类被动触发；数据驱动，零 per-ruleset 硬编码）。
    SemanticTrigger,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DueStatus {
    #[default]
    Open,
    Resolved,
    Waived,
}

// ===== Pure renderer pre-seeded for B3 (track projection semantic line) =====

/// Renders a one-line semantic status for a kernel resource track at its
/// current value, from the track's `thresholds` (zone located via `at` +
/// `direction`, mirroring the consumption in trpg-mechanics) and `zero_means`.
/// `loss_in_one_go` thresholds never locate a zone (single-application
/// magnitude semantics) but are appended as a supplementary note. Returns
/// None when nothing semantic is renderable (fail-closed: the caller falls
/// back to the bare number). Plain text, single line, no emoji/markup.
pub fn track_semantic_line(track: &serde_json::Value, current: i32) -> Option<String> {
    let id = track.get("id").and_then(|v| v.as_str()).unwrap_or("track");
    let zero_means = track
        .get("zero_means")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if current == 0 {
        if let Some(z) = zero_means {
            return Some(format!("{id}: 0 —— {}", truncate_chars(z, 40)));
        }
    }
    let cur = i64::from(current);
    let mut active: Option<(i64, String)> = None; // (distance to boundary, consequence)
    let mut nearest: Option<(i64, i64, String)> = None; // (distance, at, consequence)
    let mut loss_notes: Vec<String> = Vec::new();
    if let Some(ths) = track.get("thresholds").and_then(|v| v.as_array()) {
        for th in ths {
            // A threshold without semantics renders nothing (fail-closed).
            let cons = match th
                .get("consequence")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(c) => truncate_chars(c, 40),
                None => continue,
            };
            if let Some(at) = th.get("at").and_then(|v| v.as_i64()) {
                // Mirror trpg-mechanics: anything not "at_or_below" is at-or-above.
                let below = th.get("direction").and_then(|v| v.as_str()) == Some("at_or_below");
                let in_zone = if below { cur <= at } else { cur >= at };
                let d = (cur - at).abs();
                if in_zone {
                    if active.as_ref().map(|(ad, _)| d < *ad).unwrap_or(true) {
                        active = Some((d, cons));
                    }
                } else if nearest.as_ref().map(|(nd, _, _)| d < *nd).unwrap_or(true) {
                    nearest = Some((d, at, cons));
                }
            } else if let Some(n) = th.get("loss_in_one_go").and_then(|v| v.as_i64()) {
                loss_notes.push(format!("单次损失≥{n}：{cons}"));
            }
        }
    }
    let mut line = if let Some((_, cons)) = active {
        format!("{id}: {current} —— {cons}")
    } else if let Some((d, at, cons)) = nearest {
        format!("{id}: {current} —— 距「{cons}」阈值({at})还差{d}")
    } else if !loss_notes.is_empty() {
        format!("{id}: {current}")
    } else if let Some(z) = zero_means {
        format!("{id}: {current} —— 归零则{}", truncate_chars(z, 40))
    } else {
        return None;
    };
    if !loss_notes.is_empty() {
        line.push_str("；");
        line.push_str(&loss_notes.join("；"));
    }
    Some(line)
}

/// Char-safe truncation (never slices mid-UTF-8) to ~`max` chars.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
#[path = "mechanics_tests.rs"]
mod tests;
