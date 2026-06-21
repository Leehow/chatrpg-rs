//! Persistent player personas (蓝图 §三). At least the three of §十 第二阶段
//! (谨慎调查者 / 战术 / 新手); the full §三 roster of six is provided so the
//! later A/B harness can vary the player, not just the GM.
//!
//! A persona is pure preference data — Lego weights, no behavioral name-branching
//! (设计理念 no-hardcode). `deliberate` reads these weights; it never matches on
//! the persona's kind.

use serde::Serialize;

/// The six persistent personas of 蓝图 §三.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum PersonaKind {
    /// 谨慎调查者 — collects evidence first, low risk, presses for concrete info.
    CautiousInvestigator,
    /// 战术玩家 — position, resources, action economy, win-rate.
    Tactical,
    /// 角色扮演者 — acts from background, relationships, emotion.
    RolePlayer,
    /// 冲动推动者 — manufactures events, low patience.
    ImpulsiveDriver,
    /// 新手玩家 — little rules knowledge, asks about feasibility.
    Novice,
    /// 规则熟练玩家 — spots illegal checks and resource errors.
    RulesLawyer,
}

impl PersonaKind {
    pub fn id(&self) -> &'static str {
        match self {
            PersonaKind::CautiousInvestigator => "CAUTIOUS_INVESTIGATOR",
            PersonaKind::Tactical => "TACTICAL",
            PersonaKind::RolePlayer => "ROLE_PLAYER",
            PersonaKind::ImpulsiveDriver => "IMPULSIVE_DRIVER",
            PersonaKind::Novice => "NOVICE",
            PersonaKind::RulesLawyer => "RULES_LAWYER",
        }
    }
}

/// Preference weights in [0,1] (蓝图 §二.1 玩家层属性). Drive candidate scoring.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PersonaWeights {
    pub risk_tolerance: f32,
    pub patience: f32,
    pub rule_mastery: f32,
    pub info_seeking: f32,
    pub roleplay_intensity: f32,
    pub aggression: f32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PlayerPersona {
    pub kind: PersonaKind,
    pub weights: PersonaWeights,
}

impl PlayerPersona {
    /// All six personas of §三, ready for the persona × seed × variant matrix.
    pub const PRESETS: [PersonaKind; 6] = [
        PersonaKind::CautiousInvestigator,
        PersonaKind::Tactical,
        PersonaKind::RolePlayer,
        PersonaKind::ImpulsiveDriver,
        PersonaKind::Novice,
        PersonaKind::RulesLawyer,
    ];

    pub fn preset(kind: PersonaKind) -> Self {
        let w = match kind {
            PersonaKind::CautiousInvestigator => PersonaWeights {
                risk_tolerance: 0.15,
                patience: 0.85,
                rule_mastery: 0.55,
                info_seeking: 0.95,
                roleplay_intensity: 0.50,
                aggression: 0.10,
            },
            PersonaKind::Tactical => PersonaWeights {
                risk_tolerance: 0.50,
                patience: 0.60,
                rule_mastery: 0.85,
                info_seeking: 0.50,
                roleplay_intensity: 0.30,
                aggression: 0.55,
            },
            PersonaKind::RolePlayer => PersonaWeights {
                risk_tolerance: 0.45,
                patience: 0.55,
                rule_mastery: 0.40,
                info_seeking: 0.55,
                roleplay_intensity: 0.95,
                aggression: 0.35,
            },
            PersonaKind::ImpulsiveDriver => PersonaWeights {
                risk_tolerance: 0.80,
                patience: 0.20,
                rule_mastery: 0.40,
                info_seeking: 0.35,
                roleplay_intensity: 0.50,
                aggression: 0.80,
            },
            PersonaKind::Novice => PersonaWeights {
                risk_tolerance: 0.40,
                patience: 0.45,
                rule_mastery: 0.10,
                info_seeking: 0.60,
                roleplay_intensity: 0.45,
                aggression: 0.30,
            },
            PersonaKind::RulesLawyer => PersonaWeights {
                risk_tolerance: 0.45,
                patience: 0.70,
                rule_mastery: 0.98,
                info_seeking: 0.65,
                roleplay_intensity: 0.25,
                aggression: 0.40,
            },
        };
        PlayerPersona { kind, weights: w }
    }
}
