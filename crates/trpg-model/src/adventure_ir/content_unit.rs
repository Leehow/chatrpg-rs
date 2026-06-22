//! Adventure IR — ContentUnit (P0-2): unified identity + hierarchy. `kind` is the
//! PUBLISHING identity (Chapter/Mission/Scene/...); `facets` say which execution
//! structures a unit participates in. A book is NOT one `module_type`; the same
//! page can be BeatFlow + Timer + Objective at once (设计评审 §三.1).
use crate::SourceRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Publishing/content identity of a unit. Open enum — new kinds extend without
/// re-typing consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    Campaign,
    Arc,
    Chapter,
    Mission,
    Phase,
    Beat,
    Scene,
    Location,
    Zone,
    Encounter,
    /// A subroutine (NET architecture, chase, dungeon delve) — not a plain scene.
    Procedure,
    Handout,
    Table,
    Ending,
}

/// Which orthogonal execution structure a unit participates in. A unit usually
/// carries several. These are STRUCTURE branches (fingerprint), never ruleset
/// branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FacetKind {
    BeatFlow,
    Spatial,
    Objective,
    Timeline,
    ClueWeb,
    SocialEncounter,
    Procedure,
    KeyedLocation,
    MissionLifecycle,
    Chaos,
    Outcome,
}

/// When a unit's existence/content is allowed to surface to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityPolicy {
    /// Visible/known from the start.
    Public,
    /// Surfaces only once unlocked by progression (anti-spoiler default).
    #[default]
    OnUnlock,
    /// GM-only; never directly surfaced.
    GmOnly,
}

/// How the unit's content/function can be moved to the player (content-gravity
/// bounds, 设计评审 §五). The Director moves the carrier, never the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPolicy {
    /// Fixed to its location; cannot be relocated.
    #[default]
    FixedLocation,
    /// A knowing NPC can carry it to the player.
    MovableNpc,
    /// A clue/object that can travel.
    MovableClue,
    /// Delivered remotely (agent message, call).
    RemoteMessage,
    /// Surfaces as a world event wherever the player is.
    WorldEvent,
    /// Physically non-portable evidence — must be reached in place.
    NonPortable,
}

/// Unified content identity + hierarchy node. The old ModuleGraph/ScenarioNode is
/// kept as a compat projection; this is the fact source for hierarchy.
/// (No `Eq`: embeds `Vec<SourceRef>`, which is only `PartialEq`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContentUnit {
    pub id: String,
    pub kind: UnitKind,
    pub title: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub facets: Vec<FacetKind>,
    #[serde(default)]
    pub source_evidence: Vec<SourceRef>,
    #[serde(default)]
    pub visibility: VisibilityPolicy,
    #[serde(default)]
    pub delivery_policy: DeliveryPolicy,
}

impl ContentUnit {
    /// Minimal constructor for a hierarchy node derived from a TOC/heading tree.
    pub fn new(id: impl Into<String>, kind: UnitKind, title: impl Into<String>) -> Self {
        ContentUnit {
            id: id.into(),
            kind,
            title: title.into(),
            parent_id: None,
            facets: Vec::new(),
            source_evidence: Vec::new(),
            visibility: VisibilityPolicy::default(),
            delivery_policy: DeliveryPolicy::default(),
        }
    }

    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn has_facet(&self, f: FacetKind) -> bool {
        self.facets.contains(&f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_anti_spoiler_and_fixed() {
        let u = ContentUnit::new("unit.foxwell", UnitKind::Scene, "Foxwell Services");
        // anti-spoiler: content surfaces only on unlock by default.
        assert_eq!(u.visibility, VisibilityPolicy::OnUnlock);
        // content-gravity is conservative by default — fixed in place.
        assert_eq!(u.delivery_policy, DeliveryPolicy::FixedLocation);
        assert!(u.parent_id.is_none());
    }

    #[test]
    fn hierarchy_via_parent() {
        let chapter = ContentUnit::new("ch.1", UnitKind::Chapter, "Part One");
        let scene = ContentUnit::new("sc.1", UnitKind::Scene, "Opening").with_parent(&chapter.id);
        assert_eq!(scene.parent_id.as_deref(), Some("ch.1"));
    }

    #[test]
    fn net_architecture_is_procedure_not_scene() {
        // golden: NET Architecture → Procedure unit kind.
        let net = ContentUnit::new("unit.net_arch", UnitKind::Procedure, "NET Architecture");
        assert_eq!(net.kind, UnitKind::Procedure);
        assert_ne!(net.kind, UnitKind::Scene);
    }

    #[test]
    fn facets_are_multi() {
        let mut u = ContentUnit::new("unit.foxwell", UnitKind::Scene, "Foxwell");
        u.facets = vec![
            FacetKind::BeatFlow,
            FacetKind::Timeline,
            FacetKind::Objective,
            FacetKind::SocialEncounter,
        ];
        assert!(u.has_facet(FacetKind::Timeline));
        assert!(u.has_facet(FacetKind::Objective));
        assert!(!u.has_facet(FacetKind::Spatial));
    }

    #[test]
    fn content_unit_roundtrips_json() {
        let u = ContentUnit::new("u", UnitKind::Mission, "Springs Eternal");
        let s = serde_json::to_string(&u).unwrap();
        let back: ContentUnit = serde_json::from_str(&s).unwrap();
        assert_eq!(u, back);
    }
}
