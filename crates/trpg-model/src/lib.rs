use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

pub mod mechanics;
pub use mechanics::*;
pub mod mechanics_render;
pub use mechanics_render::*;
pub mod outcome_fields;
pub mod table_dice_policy;
pub use table_dice_policy::*;
pub mod observability;
pub use observability::*;

pub const PROJECT_SCHEMA_VERSION: &str = "chatrpg.project_bundle.v1";
pub const RULE_SCHEMA_VERSION: &str = "chatrpg.rule_bundle.v1";
pub const MODULE_SCHEMA_VERSION: &str = "chatrpg.module_bundle.v1";
pub const GM_ONBOARDING_SCHEMA_VERSION: &str = "chatrpg.gm_onboarding_bundle.v1";
pub const RULE_STEWARD_SCHEMA_VERSION: &str = "chatrpg.rule_steward.v1";
pub const CHARACTER_ONBOARDING_SCHEMA_VERSION: &str = "chatrpg.character_onboarding_pack.v1";

// R5 postprocess 生命周期状态机（turns.pp_lifecycle 列的取值，集中常量）：
// streaming → critical_done → complete。与 turns.postprocess_status（ready/awaiting）
// 正交：后者是 finalize 终态，前者是回合后处理的临界/重活落账进度（高水位守卫据此）。
pub const PP_STREAMING: &str = "streaming";
pub const PP_CRITICAL_DONE: &str = "critical_done";
pub const PP_COMPLETE: &str = "complete";

/// 生命周期阶段的有序秩（守卫用：>= critical_done 即可继续，未知值排最低 fail-closed）。
pub fn pp_lifecycle_rank(phase: &str) -> u8 {
    match phase {
        PP_STREAMING => 1,
        PP_CRITICAL_DONE => 2,
        PP_COMPLETE => 3,
        _ => 0, // 未知/旧值/空 → 最低，守卫视作未达 critical（fail-closed 多等不误读）
    }
}

pub fn sha256_hex(input: impl AsRef<[u8]>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_ref());
    format!("sha256:{:x}", hasher.finalize())
}

pub fn stable_json_hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    sha256_hex(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Rulebook,
    Module,
    Unknown,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Rulebook => "rulebook",
            SourceKind::Module => "module",
            SourceKind::Unknown => "unknown",
        }
    }
}

impl Default for SourceKind {
    fn default() -> Self { Self::Unknown }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceDocument {
    pub id: Uuid,
    pub source_id: String,
    pub source_kind: SourceKind,
    pub title: String,
    pub file_path: String,
    pub markdown_path: Option<String>,
    pub source_hash: String,
    pub parse_config_hash: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DocumentBoundingBox {
    pub page: Option<u32>,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DocumentChunk {
    pub chunk_id: String,
    pub text: String,
    #[serde(default)]
    pub full_text: String,
    #[serde(default)]
    pub page_numbers: Vec<u32>,
    #[serde(default)]
    pub element_types: Vec<String>,
    #[serde(default)]
    pub heading_context: Vec<String>,
    pub token_estimate: Option<u32>,
    #[serde(default)]
    pub is_oversized: bool,
    #[serde(default)]
    pub bounding_boxes: Vec<DocumentBoundingBox>,
    #[serde(default)]
    pub text_hash: Option<String>,
    #[serde(default)]
    pub clean_status: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlainTextBook {
    pub source_id: String,
    pub title: String,
    pub source_hash: String,
    pub pages: Vec<PageText>,
    #[serde(default)]
    pub chunks: Vec<DocumentChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SourceIndex {
    pub sources: Vec<SourceDocumentRef>,
    pub anchors: Vec<SourceAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceDocumentRef {
    pub source_id: String,
    pub title: String,
    pub source_kind: SourceKind,
    pub source_hash: String,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SourceAnchor {
    pub anchor_id: String,
    pub source_id: String,
    pub page: Option<u32>,
    pub section_path: Vec<String>,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
    pub text_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct SourceRef {
    pub source_id: String,
    pub page: Option<u32>,
    pub anchor_id: Option<String>,
    pub section_path: Vec<String>,
    pub char_start: Option<usize>,
    pub char_end: Option<usize>,
    pub text_hash: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CacheZone {
    Prefix,
    PinnedMiddle,
    DynamicTail,
    NeverPrompt,
}

impl CacheZone {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheZone::Prefix => "prefix",
            CacheZone::PinnedMiddle => "pinned_middle",
            CacheZone::DynamicTail => "dynamic_tail",
            CacheZone::NeverPrompt => "never_prompt",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    PlayerVisible,
    GmOnly,
    NpcPrivate,
    SystemOnly,
}

impl Visibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::PlayerVisible => "player_visible",
            Visibility::GmOnly => "gm_only",
            Visibility::NpcPrivate => "npc_private",
            Visibility::SystemOnly => "system_only",
        }
    }
}

impl Default for Visibility {
    fn default() -> Self { Self::GmOnly }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    Immutable,
    RarelyChanged,
    SceneStable,
    TurnDynamic,
    Ephemeral,
}


impl Default for Stability {
    fn default() -> Self { Self::RarelyChanged }
}

impl Stability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stability::Immutable => "immutable",
            Stability::RarelyChanged => "rarely_changed",
            Stability::SceneStable => "scene_stable",
            Stability::TurnDynamic => "turn_dynamic",
            Stability::Ephemeral => "ephemeral",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    EngineProtocol,
    OutputSchema,
    ToolProtocol,
    RulesetResidentCore,
    RulesetWorldStyle,
    RulesetDirectorPolicy,
    RulesetActionRouter,
    RulesetIndex,
    RulesetCharacterKernel,
    RulesetOnboarding,
    RuleStewardKernel,
    MechanicsCatalogIndex,
    RuleAgentRun,
    RuleKernelPatch,
    RuleEntityLocator,
    MechanicalSourcePack,
    PlayabilityGateReport,
    CharacterOnboardingPack,
    CharacterCreationFlow,
    CharacterOptionCatalog,
    DerivedFormulaPack,
    StarterCharacterPack,
    GameIdentity,
    PlayLoop,
    BookLocator,
    LookupRecipe,
    GmOnboarding,
    BookLocatorSummary,
    ColdDataLocator,
    LearnedPacket,
    LookupResult,
    Ruling,
    RulingLog,
    RulePackage,
    ProcedureDetail,
    ProcedureVariant,
    ParameterFamily,
    ParameterEntry,
    DomainActor,
    DomainObject,
    ObjectDefinition,
    ObjectInstance,
    ObjectAffordance,
    ObjectInteractionContract,
    ObjectPatch,
    ObjectEvent,
    ObjectGraph,
    DomainAbility,
    AbilityDefinition,
    AbilityInstance,
    AbilityTriggerBinding,
    AbilityActivationContract,
    AbilityGraph,
    RuleBindingPacket,
    SemanticClassification,
    MaterializationDemand,
    SourceEvidenceBundle,
    ExtractionRun,
    BindingVerification,
    ActorRuntimeBinding,
    ObjectRuntimeBinding,
    AbilityRuntimeBinding,
    MaterializationGraph,
    PlayerValueClaim,
    PlayerValueVerification,
    TableOverrideAgreement,
    RefereeRuling,
    MechanicalLedger,
    AttackResolutionContract,
    DamagePacket,
    RollPlan,
    EffectResolutionPacket,
    ParameterImpact,
    ActorMechanicalState,
    CombatRoundAction,
    CombatRoundTransition,
    ContestProfile,
    OppositionProfile,
    ContestResolution,
    ContestGraph,
    MechanicsSearchSkill,
    MechanicsQueryPlan,
    ParameterFacetBinding,
    MechanicsSearchTrace,
    MechanicsSearchGraph,
    ParameterFacetExecution,
    GenericParameterState,
    RulesetMechanicalProfile,
    DomainInfo,
    DomainPlace,
    DomainScenario,
    StateTrait,
    StateTrack,
    StateStatus,
    StateAspect,
    StateModifier,
    ScenarioNode,
    MissionStatic,
    Clue,
    Handout,
    NpcStatic,
    LocationStatic,
    ChapterStatic,
    ModuleSpine,
    ModuleStyle,
    ModuleSpecificRule,
    ModuleOverview,
    CurrentSessionPacket,
    SessionSummary,
    MemorySnapshot,
    MemoryFact,
    MemoryEvent,
    SceneStatic,
    AgentPlan,
    AgentAdvice,
    CheckContract,
    PendingCheck,
    InteractionGate,
    DiceRoll,
    StateFrame,
    FrameEvent,
    FrameCompaction,
    CombatFrame,
    CombatEvent,
    EffectContract,
    CombatProfile,
    ReactionWindow,
    DirectorPolicy,
    ActionableSituationBrief,
    ClueBoard,
    ConsequenceContract,
    SpotlightState,
    ClockEvent,
    WorldTime,
    WorldEvent,
    TimeAdvance,
    ScheduledEvent,
    TimeAnchor,
    WorldState,
    RetrievedMemory,
    RecentTranscript,
    CurrentInput,
}

impl BlockKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockKind::EngineProtocol => "engine_protocol",
            BlockKind::OutputSchema => "output_schema",
            BlockKind::ToolProtocol => "tool_protocol",
            BlockKind::RulesetResidentCore => "ruleset_resident_core",
            BlockKind::RulesetWorldStyle => "ruleset_world_style",
            BlockKind::RulesetDirectorPolicy => "ruleset_director_policy",
            BlockKind::RulesetActionRouter => "ruleset_action_router",
            BlockKind::RulesetIndex => "ruleset_index",
            BlockKind::RulesetCharacterKernel => "ruleset_character_kernel",
            BlockKind::RulesetOnboarding => "ruleset_onboarding",
            BlockKind::RuleStewardKernel => "rule_steward_kernel",
            BlockKind::MechanicsCatalogIndex => "mechanics_catalog_index",
            BlockKind::RuleAgentRun => "rule_agent_run",
            BlockKind::RuleKernelPatch => "rule_kernel_patch",
            BlockKind::RuleEntityLocator => "rule_entity_locator",
            BlockKind::MechanicalSourcePack => "mechanical_source_pack",
            BlockKind::PlayabilityGateReport => "playability_gate_report",
            BlockKind::CharacterOnboardingPack => "character_onboarding_pack",
            BlockKind::CharacterCreationFlow => "character_creation_flow",
            BlockKind::CharacterOptionCatalog => "character_option_catalog",
            BlockKind::DerivedFormulaPack => "derived_formula_pack",
            BlockKind::StarterCharacterPack => "starter_character_pack",
            BlockKind::GameIdentity => "game_identity",
            BlockKind::PlayLoop => "play_loop",
            BlockKind::BookLocator => "book_locator",
            BlockKind::LookupRecipe => "lookup_recipe",
            BlockKind::GmOnboarding => "gm_onboarding",
            BlockKind::BookLocatorSummary => "book_locator_summary",
            BlockKind::ColdDataLocator => "cold_data_locator",
            BlockKind::LearnedPacket => "learned_packet",
            BlockKind::LookupResult => "lookup_result",
            BlockKind::Ruling => "ruling",
            BlockKind::RulingLog => "ruling_log",
            BlockKind::RulePackage => "rule_package",
            BlockKind::ProcedureDetail => "procedure_detail",
            BlockKind::ProcedureVariant => "procedure_variant",
            BlockKind::ParameterFamily => "parameter_family",
            BlockKind::ParameterEntry => "parameter_entry",
            BlockKind::DomainActor => "domain_actor",
            BlockKind::DomainObject => "domain_object",
            BlockKind::ObjectDefinition => "object_definition",
            BlockKind::ObjectInstance => "object_instance",
            BlockKind::ObjectAffordance => "object_affordance",
            BlockKind::ObjectInteractionContract => "object_interaction_contract",
            BlockKind::ObjectPatch => "object_patch",
            BlockKind::ObjectEvent => "object_event",
            BlockKind::ObjectGraph => "object_graph",
            BlockKind::DomainAbility => "domain_ability",
            BlockKind::AbilityDefinition => "ability_definition",
            BlockKind::AbilityInstance => "ability_instance",
            BlockKind::AbilityTriggerBinding => "ability_trigger_binding",
            BlockKind::AbilityActivationContract => "ability_activation_contract",
            BlockKind::AbilityGraph => "ability_graph",
            BlockKind::RuleBindingPacket => "rule_binding_packet",
            BlockKind::SemanticClassification => "semantic_classification",
            BlockKind::MaterializationDemand => "materialization_demand",
            BlockKind::SourceEvidenceBundle => "source_evidence_bundle",
            BlockKind::ExtractionRun => "extraction_run",
            BlockKind::BindingVerification => "binding_verification",
            BlockKind::ActorRuntimeBinding => "actor_runtime_binding",
            BlockKind::ObjectRuntimeBinding => "object_runtime_binding",
            BlockKind::AbilityRuntimeBinding => "ability_runtime_binding",
            BlockKind::MaterializationGraph => "materialization_graph",
            BlockKind::PlayerValueClaim => "player_value_claim",
            BlockKind::PlayerValueVerification => "player_value_verification",
            BlockKind::TableOverrideAgreement => "table_override_agreement",
            BlockKind::RefereeRuling => "referee_ruling",
            BlockKind::MechanicalLedger => "mechanical_ledger",
            BlockKind::AttackResolutionContract => "attack_resolution_contract",
            BlockKind::DamagePacket => "damage_packet",
            BlockKind::RollPlan => "roll_plan",
            BlockKind::EffectResolutionPacket => "effect_resolution_packet",
            BlockKind::ParameterImpact => "parameter_impact",
            BlockKind::ActorMechanicalState => "actor_mechanical_state",
            BlockKind::CombatRoundAction => "combat_round_action",
            BlockKind::CombatRoundTransition => "combat_round_transition",
            BlockKind::ContestProfile => "contest_profile",
            BlockKind::OppositionProfile => "opposition_profile",
            BlockKind::ContestResolution => "contest_resolution",
            BlockKind::ContestGraph => "contest_graph",
            BlockKind::MechanicsSearchSkill => "mechanics_search_skill",
            BlockKind::MechanicsQueryPlan => "mechanics_query_plan",
            BlockKind::ParameterFacetBinding => "parameter_facet_binding",
            BlockKind::MechanicsSearchTrace => "mechanics_search_trace",
            BlockKind::MechanicsSearchGraph => "mechanics_search_graph",
            BlockKind::ParameterFacetExecution => "parameter_facet_execution",
            BlockKind::GenericParameterState => "generic_parameter_state",
            BlockKind::RulesetMechanicalProfile => "ruleset_mechanical_profile",
            BlockKind::DomainInfo => "domain_info",
            BlockKind::DomainPlace => "domain_place",
            BlockKind::DomainScenario => "domain_scenario",
            BlockKind::StateTrait => "state_trait",
            BlockKind::StateTrack => "state_track",
            BlockKind::StateStatus => "state_status",
            BlockKind::StateAspect => "state_aspect",
            BlockKind::StateModifier => "state_modifier",
            BlockKind::ScenarioNode => "scenario_node",
            BlockKind::MissionStatic => "mission_static",
            BlockKind::Clue => "clue",
            BlockKind::Handout => "handout",
            BlockKind::NpcStatic => "npc_static",
            BlockKind::LocationStatic => "location_static",
            BlockKind::ChapterStatic => "chapter_static",
            BlockKind::ModuleSpine => "module_spine",
            BlockKind::ModuleStyle => "module_style",
            BlockKind::ModuleSpecificRule => "module_specific_rule",
            BlockKind::ModuleOverview => "module_overview",
            BlockKind::CurrentSessionPacket => "current_session_packet",
            BlockKind::SessionSummary => "session_summary",
            BlockKind::MemorySnapshot => "memory_snapshot",
            BlockKind::MemoryFact => "memory_fact",
            BlockKind::MemoryEvent => "memory_event",
            BlockKind::SceneStatic => "scene_static",
            BlockKind::AgentPlan => "agent_plan",
            BlockKind::AgentAdvice => "agent_advice",
            BlockKind::CheckContract => "check_contract",
            BlockKind::PendingCheck => "pending_check",
            BlockKind::InteractionGate => "interaction_gate",
            BlockKind::DiceRoll => "dice_roll",
            BlockKind::StateFrame => "state_frame",
            BlockKind::FrameEvent => "frame_event",
            BlockKind::FrameCompaction => "frame_compaction",
            BlockKind::CombatFrame => "combat_frame",
            BlockKind::CombatEvent => "combat_event",
            BlockKind::EffectContract => "effect_contract",
            BlockKind::CombatProfile => "combat_profile",
            BlockKind::ReactionWindow => "reaction_window",
            BlockKind::DirectorPolicy => "director_policy",
            BlockKind::ActionableSituationBrief => "actionable_situation_brief",
            BlockKind::ClueBoard => "clue_board",
            BlockKind::ConsequenceContract => "consequence_contract",
            BlockKind::SpotlightState => "spotlight_state",
            BlockKind::ClockEvent => "clock_event",
            BlockKind::WorldTime => "world_time",
            BlockKind::WorldEvent => "world_event",
            BlockKind::TimeAdvance => "time_advance",
            BlockKind::ScheduledEvent => "scheduled_event",
            BlockKind::TimeAnchor => "time_anchor",
            BlockKind::WorldState => "world_state",
            BlockKind::RetrievedMemory => "retrieved_memory",
            BlockKind::RecentTranscript => "recent_transcript",
            BlockKind::CurrentInput => "current_input",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScopeType {
    Global,
    Ruleset,
    Module,
    Campaign,
    Chapter,
    Mission,
    Location,
    Npc,
    Session,
    Scene,
    Turn,
    Material,
    Character,
    Object,
}

impl ScopeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeType::Global => "global",
            ScopeType::Ruleset => "ruleset",
            ScopeType::Module => "module",
            ScopeType::Campaign => "campaign",
            ScopeType::Chapter => "chapter",
            ScopeType::Mission => "mission",
            ScopeType::Location => "location",
            ScopeType::Npc => "npc",
            ScopeType::Session => "session",
            ScopeType::Scene => "scene",
            ScopeType::Turn => "turn",
            ScopeType::Material => "material",
            ScopeType::Character => "character",
            ScopeType::Object => "object",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Scope {
    pub scope_type: ScopeType,
    pub scope_id: String,
}

impl Default for Scope {
    fn default() -> Self { Self { scope_type: ScopeType::Global, scope_id: "global".to_string() } }
}

impl Scope {
    pub fn global() -> Self {
        Self { scope_type: ScopeType::Global, scope_id: "global".to_string() }
    }
    pub fn ruleset(id: impl Into<String>) -> Self {
        Self { scope_type: ScopeType::Ruleset, scope_id: id.into() }
    }
    pub fn module(id: impl Into<String>) -> Self {
        Self { scope_type: ScopeType::Module, scope_id: id.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", content = "value", rename_all = "snake_case")]
pub enum BlockContent {
    Markdown(String),
    Text(String),
    Json(serde_json::Value),
    Procedure(ProcedureDef),
    Asset(serde_json::Value),
    ScenarioNode(ScenarioNode),
    DirectorPolicy(DirectorPolicy),
}

impl BlockContent {
    pub fn render_text(&self) -> String {
        match self {
            BlockContent::Markdown(s) | BlockContent::Text(s) => s.clone(),
            BlockContent::Json(v) | BlockContent::Asset(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
            BlockContent::Procedure(p) => serde_json::to_string_pretty(p).unwrap_or_default(),
            BlockContent::ScenarioNode(n) => serde_json::to_string_pretty(n).unwrap_or_default(),
            BlockContent::DirectorPolicy(p) => serde_json::to_string_pretty(p).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextBlock {
    pub block_id: String,
    pub kind: BlockKind,
    pub title: String,
    pub content: BlockContent,
    pub visibility: Visibility,
    pub stability: Stability,
    pub cache_zone: CacheZone,
    pub scope: Scope,
    pub priority: i32,
    pub version: u32,
    pub tags: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub dependencies: Vec<String>,
    pub content_hash: String,
    pub token_estimate: Option<u32>,
    pub expires_at_turn: Option<String>,
    pub expires_at_scene: Option<String>,
    pub load_reason: Option<String>,
}

impl ContextBlock {
    pub fn new(
        block_id: impl Into<String>,
        kind: BlockKind,
        title: impl Into<String>,
        content: BlockContent,
        visibility: Visibility,
        stability: Stability,
        cache_zone: CacheZone,
        scope: Scope,
        priority: i32,
    ) -> Self {
        let block_id = block_id.into();
        let title = title.into();
        let content_hash = stable_json_hash(&content);
        let token_estimate = Some((content.render_text().chars().count() as u32 / 4).max(1));
        Self {
            block_id,
            kind,
            title,
            content,
            visibility,
            stability,
            cache_zone,
            scope,
            priority,
            version: 1,
            tags: vec![],
            source_refs: vec![],
            dependencies: vec![],
            content_hash,
            token_estimate,
            expires_at_turn: None,
            expires_at_scene: None,
            load_reason: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoadPredicate {
    ActiveRuleset { ruleset_id: String },
    ActiveModule { module_id: String },
    ActiveCampaign { campaign_id: String },
    ActiveChapter { chapter_id: String },
    ActiveMission { mission_id: String },
    ActiveScene { scene_id: String },
    ActiveLocation { location_id: String },
    ActiveNpc { npc_id: String },
    PendingMaterialRef { material_id: String },
    RecentIntentTag { tag: String },
    TrackAtLeast { track_id: String, value: i32 },
    HasStatus { actor_id: String, status_id: String },
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialType {
    RulePackage,
    Procedure,
    CharacterTemplate,
    CharacterOnboardingPack,
    CharacterCreationFlow,
    CharacterOptionCatalog,
    DerivedFormulaPack,
    StarterCharacterPack,
    RuleKernel,
    RuleKernelPatch,
    RuleEntityLocator,
    MechanicalSourcePack,
    PlayabilityGateReport,
    GmOnboarding,
    BookLocator,
    ColdDataLocator,
    LearnedPacket,
    LookupRecipe,
    CurrentSessionPacket,
    Ruling,
    AssetIndex,
    Spell,
    AbilityDefinition,
    AbilityInstance,
    RuleBindingPacket,
    ObjectDefinition,
    ObjectInstance,
    Item,
    Weapon,
    Armor,
    Tool,
    Device,
    Vehicle,
    QuestItem,
    Monster,
    Npc,
    Location,
    Scene,
    Mission,
    Chapter,
    Clue,
    Handout,
    Encounter,
    ModuleSpecificRule,
    DirectorPolicy,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MaterialIndexEntry {
    pub material_id: String,
    pub material_type: MaterialType,
    pub title: String,
    pub summary: String,
    pub default_cache_zone: CacheZone,
    pub visibility: Visibility,
    pub stability: Stability,
    pub source_refs: Vec<SourceRef>,
    pub dependencies: Vec<String>,
    pub load_when: Vec<LoadPredicate>,
    pub extracted_block_id: Option<String>,
    pub estimated_tokens: Option<u32>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentType {
    CoreRulebook,
    RulesetFamily,
    Supplement,
    ScenarioCollection,
    Campaign,
    OneShot,
    AssetCatalog,
    Unknown,
}

impl Default for DocumentType {
    fn default() -> Self { Self::Unknown }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ParsedRuleset {
    pub ruleset_id: String,
    pub title: String,
    pub document_type: Option<DocumentType>,
    pub ruleset_family: Option<RulesetFamily>,
    pub ruleset_profiles: Vec<RulesetProfile>,
    pub option_modules: Vec<OptionModule>,
    pub compatibility_policy: Option<CompatibilityPolicy>,
    pub domain: DomainModel,
    pub state: StateModel,
    pub director: DirectorModel,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RulesetFamily {
    pub family_id: String,
    pub title: String,
    pub core_books: Vec<String>,
    pub supplements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RulesetProfile {
    pub profile_id: String,
    pub title: String,
    pub enabled_modules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct OptionModule {
    pub module_id: String,
    pub title: String,
    pub summary: String,
    pub default_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CompatibilityPolicy {
    pub notes: Vec<String>,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DomainModel {
    pub actors: Vec<serde_json::Value>,
    pub objects: Vec<serde_json::Value>,
    pub abilities: Vec<serde_json::Value>,
    pub info: Vec<serde_json::Value>,
    pub places: Vec<serde_json::Value>,
    pub scenarios: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StateModel {
    pub traits: Vec<TraitDef>,
    pub tracks: Vec<TrackDef>,
    pub statuses: Vec<StatusDef>,
    pub aspects: Vec<AspectDef>,
    pub modifiers: Vec<ModifierDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TraitDef { pub trait_id: String, pub title: String, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TrackDef { pub track_id: String, pub title: String, pub min: Option<i32>, pub max: Option<i32>, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StatusDef { pub status_id: String, pub title: String, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AspectDef { pub aspect_id: String, pub title: String, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ModifierDef { pub modifier_id: String, pub title: String, pub data: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorModel {
    pub policies: Vec<DirectorPolicy>,
    pub output_contracts: Vec<OutputContract>,
    pub state_proposal_contract: Option<StateProposalContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorPolicy {
    pub policy_id: String,
    pub title: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct OutputContract { pub contract_id: String, pub content: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StateProposalContract { pub content: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ProcedureRegistry {
    pub procedures: Vec<ProcedureDef>,
    pub variants: Vec<ProcedureVariant>,
    pub effects: Vec<MechanicalEffectDef>,
    pub hooks: Vec<HookDef>,
    pub action_templates: Vec<ActionTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum RollModel {
    D20 { dc: Option<i32>, allow_advantage: bool },
    PercentileRollUnder,
    BrpCharacteristic { default_multiplier: i32 },
    BrpResistance { auto_difference: Option<i32> },
    D10StatSkill,
    Sw2d6,
    Triangle6d4,
    DicePool { dice: String, success_threshold: i32 },
    Automatic,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProcedureDef {
    pub procedure_id: String,
    pub title: String,
    pub summary: String,
    pub roll_model: RollModel,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub special: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ProcedureVariant { pub variant_id: String, pub procedure_id: String, pub title: String, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MechanicalEffectDef { pub effect_id: String, pub title: String, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct HookDef { pub hook_id: String, pub title: String, pub data: serde_json::Value }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActionTemplate { pub action_id: String, pub title: String, pub data: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterTemplate {
    pub template_id: String,
    pub ruleset_id: String,
    pub title: String,
    pub source_refs: Vec<SourceRef>,
    pub sections: Vec<CharacterSection>,
    pub fields: Vec<CharacterField>,
    pub derived_values: Vec<DerivedValue>,
    pub creation_flow: Vec<CreationStep>,
    pub validation_rules: Vec<CharacterValidationRule>,
    pub llm_creation_policy: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterSection {
    pub section_id: String,
    pub title: String,
    pub field_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterField {
    pub field_id: String,
    pub title: String,
    pub field_type: String,
    pub required: bool,
    pub repeatable: bool,
    pub choices_material_id: Option<String>,
    pub default_value: Option<serde_json::Value>,
    pub visibility: Option<Visibility>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DerivedValue {
    pub field_id: String,
    pub formula: String,
    pub depends_on: Vec<String>,
    pub evaluator: String,
    pub notes: Option<String>,
    // §4 chargen-spec fields (machine-evaluable by trpg-formula). All optional +
    // backward-compatible: old {field_id,formula,...} entries still parse. When
    // present, `expr` (with {{token}} placeholders) + lookup_tables/hybrid let the
    // evaluator compute the value deterministically instead of reading prose.
    #[serde(default)] pub role: Option<String>,                 // attribute|skill|resource|resource_max|background
    #[serde(default)] pub input_kind: Option<String>,           // player|derived|hybrid
    #[serde(default)] pub recompute: Option<String>,            // live|once
    #[serde(default)] pub result_type: Option<String>,          // int|float|dice_or_int
    #[serde(default)] pub expr: Option<String>,                 // machine expr w/ {{ids}}; preferred over `formula`
    #[serde(default)] pub attr_derived: Option<String>,         // hybrid: characteristic-derived segment (e.g. floor({{dex}}/2))
    #[serde(default)] pub base: Option<serde_json::Value>,      // hybrid: fixed base
    #[serde(default)] pub allocations: Vec<serde_json::Value>,  // hybrid: player point-allocation segments
    #[serde(default)] pub lookup_tables: serde_json::Value,     // inline {name:{ranges:[{min,max,value}]}}
    #[serde(default)] pub min: Option<serde_json::Value>,
    #[serde(default)] pub max: Option<serde_json::Value>,
    #[serde(default)] pub clamp_max: Option<serde_json::Value>,
    #[serde(default)] pub source_ref: Option<serde_json::Value>,
    #[serde(default)] pub status: Option<String>,               // source_backed|provisional
    /// Formula classification tier (N1).
    /// - `"exact_executable"`: LLM extracted and source-backed; can drive exact resolution.
    /// - `"provisional_seed"`: deterministic first-play seed injected by Rule Steward/parser;
    ///   usable for context only — combat/effect executor must NOT use for exact dice binding.
    /// - `"operational_abstract"`: extracted but missing numeric values; operational guide only.
    /// Absent (None) → treat as `"exact_executable"` for backward-compat with old packs.
    #[serde(default)] pub tier: Option<String>,
}

impl DerivedValue {
    /// Returns true when this formula is a provisional seed injected by the
    /// Rule Steward or parser — not an exact LLM-extracted source-backed formula.
    /// Executors must NOT use provisional seeds for exact dice binding.
    pub fn is_provisional_seed(&self) -> bool {
        self.tier.as_deref() == Some("provisional_seed")
    }

    /// Returns true when this formula is operational-abstract (extracted but
    /// missing concrete numeric values). Also not safe for exact binding.
    pub fn is_operational_abstract(&self) -> bool {
        self.tier.as_deref() == Some("operational_abstract")
    }

    /// Returns true when this formula can drive exact mechanical resolution.
    /// Absent tier is treated as exact for backward-compat.
    pub fn is_exact_executable(&self) -> bool {
        !self.is_provisional_seed() && !self.is_operational_abstract()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CreationStep {
    pub step_id: String,
    pub title: String,
    pub required: bool,
    pub prompt: Option<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterValidationRule {
    pub rule_id: String,
    pub severity: String,
    pub description: String,
    pub expression: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CharacterCreationMode {
    Pregenerated,
    QuickStart,
    Guided,
    Detailed,
    ImportExistingSheet,
    Randomized,
    HighLevel,
}

impl Default for CharacterCreationMode {
    fn default() -> Self { Self::Guided }
}

impl CharacterCreationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pregenerated => "pregenerated",
            Self::QuickStart => "quick_start",
            Self::Guided => "guided",
            Self::Detailed => "detailed",
            Self::ImportExistingSheet => "import_existing_sheet",
            Self::Randomized => "randomized",
            Self::HighLevel => "high_level",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterCreationEdge {
    pub from_step_id: String,
    pub to_step_id: String,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterCreationFlow {
    pub flow_id: String,
    pub ruleset_id: String,
    pub title: String,
    #[serde(default)]
    pub mode: CharacterCreationMode,
    #[serde(default)]
    pub supported_modes: Vec<CharacterCreationMode>,
    #[serde(default)]
    pub steps: Vec<CreationStep>,
    #[serde(default)]
    pub decision_graph: Vec<CharacterCreationEdge>,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    #[serde(default)]
    pub validation_profile: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterOptionCatalog {
    pub catalog_id: String,
    pub ruleset_id: String,
    pub title: String,
    #[serde(default)]
    pub option_groups: Vec<CharacterOptionGroup>,
    #[serde(default)]
    pub option_locators: Vec<BookLocatorEntry>,
    #[serde(default)]
    pub compatibility_rules: Vec<CharacterValidationRule>,
    #[serde(default)]
    pub module_recommendations: Vec<ModuleCharacterRecommendation>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterOptionGroup {
    pub group_id: String,
    pub title: String,
    pub category: String,
    #[serde(default)]
    pub starter_legal: bool,
    #[serde(default)]
    pub options: Vec<CharacterOptionSummary>,
    #[serde(default)]
    pub locators: Vec<BookLocatorEntry>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterOptionSummary {
    pub option_id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DerivedFormulaPack {
    pub pack_id: String,
    pub ruleset_id: String,
    #[serde(default)]
    pub formulas: Vec<DerivedValue>,
    #[serde(default)]
    pub precedence_rules: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    #[serde(default)]
    pub validation_report: ValidationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StarterCharacterPack {
    pub pack_id: String,
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
    #[serde(default)]
    pub pregens: Vec<PregenCharacterRef>,
    #[serde(default)]
    pub archetypes: Vec<RecommendedArchetype>,
    #[serde(default)]
    pub creation_shortcuts: Vec<CreationShortcut>,
    #[serde(default)]
    pub module_fit_notes: Vec<ModuleFitNote>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PregenCharacterRef {
    pub pregen_id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub sheet_ref: Option<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RecommendedArchetype {
    pub archetype_id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub fit_tags: Vec<String>,
    #[serde(default)]
    pub required_option_refs: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CreationShortcut {
    pub shortcut_id: String,
    pub title: String,
    pub mode: CharacterCreationMode,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ModuleFitNote {
    #[serde(default)]
    pub module_id: Option<String>,
    pub note: String,
    #[serde(default)]
    pub visibility: Option<Visibility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ModuleCharacterRecommendation {
    #[serde(default)]
    pub module_id: Option<String>,
    pub recommendation: String,
    #[serde(default)]
    pub fit_tags: Vec<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterRuntimeBinding {
    pub sheet_path: String,
    pub runtime_path: String,
    #[serde(default)]
    pub binding_kind: String,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CharacterOnboardingPack {
    pub pack_id: String,
    pub ruleset_id: String,
    pub title: String,
    pub sheet_template: CharacterTemplate,
    #[serde(default)]
    pub creation_flows: Vec<CharacterCreationFlow>,
    #[serde(default)]
    pub option_catalogs: Vec<CharacterOptionCatalog>,
    #[serde(default)]
    pub derived_formula_pack: DerivedFormulaPack,
    #[serde(default)]
    pub starter_character_pack: StarterCharacterPack,
    #[serde(default)]
    pub import_mapping_profile: serde_json::Value,
    #[serde(default)]
    pub validation_profile: serde_json::Value,
    #[serde(default)]
    pub runtime_binding_profile: serde_json::Value,
    #[serde(default)]
    pub runtime_bindings: Vec<CharacterRuntimeBinding>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    #[serde(default)]
    pub validation_report: ValidationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleNeed {
    pub need_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
    #[serde(default)]
    pub scene_id: Option<String>,
    pub need_kind: RuleNeedKind,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub player_action_summary: String,
    #[serde(default)]
    pub involved_refs: Vec<String>,
    #[serde(default)]
    pub missing_facets: Vec<String>,
    #[serde(default)]
    pub current_contract: Option<serde_json::Value>,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub urgency: RuleUrgency,
    #[serde(default)]
    pub allowed_outputs: Vec<RuleAssistOutputKind>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleNeedKind {
    CoreResolutionModel,
    CheckProcedure,
    AttackProcedure,
    DamageProcedure,
    DefenseOrArmorProcedure,
    SaveOrResistanceProcedure,
    AbilityOrSpellUse,
    ObjectInteraction,
    ResourceOrCondition,
    CharacterSheetField,
    CharacterCreation,
    NpcOrMonsterStatBlock,
    SceneOrModuleRule,
    VisibilityOrSpoilerDecision,
    LearningAudit,
    Bp1KernelReview,
    GeneralRuleQuery,
}

impl Default for RuleNeedKind { fn default() -> Self { Self::GeneralRuleQuery } }

impl RuleNeedKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CoreResolutionModel => "core_resolution_model",
            Self::CheckProcedure => "check_procedure",
            Self::AttackProcedure => "attack_procedure",
            Self::DamageProcedure => "damage_procedure",
            Self::DefenseOrArmorProcedure => "defense_or_armor_procedure",
            Self::SaveOrResistanceProcedure => "save_or_resistance_procedure",
            Self::AbilityOrSpellUse => "ability_or_spell_use",
            Self::ObjectInteraction => "object_interaction",
            Self::ResourceOrCondition => "resource_or_condition",
            Self::CharacterSheetField => "character_sheet_field",
            Self::CharacterCreation => "character_creation",
            Self::NpcOrMonsterStatBlock => "npc_or_monster_stat_block",
            Self::SceneOrModuleRule => "scene_or_module_rule",
            Self::VisibilityOrSpoilerDecision => "visibility_or_spoiler_decision",
            Self::LearningAudit => "learning_audit",
            Self::Bp1KernelReview => "bp1_kernel_review",
            Self::GeneralRuleQuery => "general_rule_query",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleUrgency { ImmediateTurn, BeforeScene, BackgroundAudit }
impl Default for RuleUrgency { fn default() -> Self { Self::ImmediateTurn } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleAssistOutputKind { ContextBlock, CheckContractPatch, ContestProfilePatch, MaterializationPatch, CharacterOnboardingPatch, Bp1PatchProposal, LearnedPacketCandidate }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleAssistStatus { SourceBackedExact, SourceBackedPartial, LearnedStable, ProvisionalTableRuling, UnresolvedNeedsSource, ConflictNeedsReview, NotARulesProblem }
impl Default for RuleAssistStatus { fn default() -> Self { Self::UnresolvedNeedsSource } }

impl RuleAssistStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SourceBackedExact => "source_backed_exact",
            Self::SourceBackedPartial => "source_backed_partial",
            Self::LearnedStable => "learned_stable",
            Self::ProvisionalTableRuling => "provisional_table_ruling",
            Self::UnresolvedNeedsSource => "unresolved_needs_source",
            Self::ConflictNeedsReview => "conflict_needs_review",
            Self::NotARulesProblem => "not_a_rules_problem",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleAnswerScope { RulesetCore, ModuleSpecific, CharacterCreation, RuntimeTurn, AuditOnly }
impl Default for RuleAnswerScope { fn default() -> Self { Self::RuntimeTurn } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleAssist {
    pub assist_id: String,
    pub need_id: String,
    #[serde(default)]
    pub status: RuleAssistStatus,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub answer_scope: RuleAnswerScope,
    #[serde(default)]
    pub gm_brief: String,
    #[serde(default)]
    pub player_safe_summary: Option<String>,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    #[serde(default)]
    pub evidence_bundle_id: Option<String>,
    #[serde(default)]
    pub context_blocks: Vec<ContextBlock>,
    #[serde(default)]
    pub search_hits: Vec<SearchHit>,
    #[serde(default)]
    pub learned_packet_candidates: Vec<String>,
    #[serde(default)]
    pub bp1_patch_proposal: Option<RuleKernelPatch>,
    #[serde(default)]
    pub unresolved_questions: Vec<RuleGap>,
    #[serde(default)]
    pub audit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleGap {
    pub gap_id: String,
    pub description: String,
    #[serde(default)]
    pub blocking: bool,
    #[serde(default)]
    pub suggested_skill: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleKernel {
    pub kernel_id: String,
    pub ruleset_id: String,
    pub version: String,
    #[serde(default)]
    pub game_identity: serde_json::Value,
    #[serde(default)]
    pub play_loop: serde_json::Value,
    #[serde(default)]
    pub dice_core: serde_json::Value,
    #[serde(default)]
    pub check_model: serde_json::Value,
    #[serde(default)]
    pub contest_models: Vec<serde_json::Value>,
    #[serde(default)]
    pub damage_effect_model: serde_json::Value,
    #[serde(default)]
    pub resource_tracks: Vec<serde_json::Value>,
    /// Per-category object/ability SCHEMAS (+ 2 worked examples each), compiled
    /// once by the reader's object pass. Each entry: {category_id, kind,
    /// schema_slots:[{slot,type,hook}], examples:[{name,slots,source_pages}],
    /// provisional_slots}. Items/spells/abilities are INSTANCES of these schemas;
    /// play-time materialization fills the typed slots using the examples as the
    /// pattern (vs free-form). Empty for rulesets parsed before this pass.
    #[serde(default)]
    pub object_schemas: Vec<serde_json::Value>,
    #[serde(default)]
    pub mechanics_catalog: Vec<MechanicEntry>,
    #[serde(default)]
    pub character_sheet_schema: serde_json::Value,
    #[serde(default)]
    pub visibility_policy: serde_json::Value,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    #[serde(default)]
    pub validation_report: ValidationReport,
    /// P0-2: combat action-economy/initiative/reaction/search strategy — replaces
    /// trpg-combat's {cyberpunk,dnd,coc,triangle,sword_world}_profile(). None →
    /// engine uses GENERIC_COMBAT_PROFILE (no per-ruleset branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combat_profile: Option<CombatProfile>,
    /// P0-2: intent→CombatMode data mapping — replaces infer_combat_mode_from_intent's
    /// ruleset_id.contains branches. None → GENERIC_COMBAT_MODE_POLICY.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combat_mode_policy: Option<CombatModePolicy>,
    /// P0-2: check-label templates — replaces combat/object contains("cyberpunk") labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_label_policy: Option<CheckLabelPolicy>,
    /// P0-2: damage/difficulty family + band + plausibility — replaces trpg-referee's
    /// five ruleset_* functions. None → GENERIC_REFEREE_BANDS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referee_value_bands: Option<RefereeValueBands>,
    /// P0-2: bare-dice qualification (e.g. "1d10" → "1d10+0") — replaces combat's
    /// hardcoded "1d10+0". None → GENERIC_DICE_QUALIFICATION.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dice_qualification: Option<DiceQualification>,
    /// P0-2 T5: per-skill section hints + field aliases — replaces trpg-material's
    /// ruleset_aliases_for contains(ruleset_name) branches. None → generic base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_profile: Option<RuleKernelSearchProfile>,
}

/// The id of the kernel resource_track that represents Hit Points, found
/// semantically (kind first, then id substring) so combat/mechanics read HP by
/// kernel data, not a hardcoded "hp_max" field name. None if no HP-like track.
pub fn hp_resource_track_id(resource_tracks: &[serde_json::Value]) -> Option<String> {
    let id_of = |t: &serde_json::Value| t.get("id").and_then(|v| v.as_str()).map(|s| s.trim().to_string());
    // 1) an explicit health-kind track
    if let Some(t) = resource_tracks.iter().find(|t| t.get("kind").and_then(|v| v.as_str()) == Some("health")) {
        if let Some(id) = id_of(t) { if !id.is_empty() { return Some(id); } }
    }
    // 2) id contains "hit_point" or equals/starts "hp"
    resource_tracks.iter().filter_map(|t| id_of(t)).find(|id| {
        let l = id.to_ascii_lowercase();
        l.contains("hit_point") || l == "hp" || l.starts_with("hp_")
    })
}

/// Pure (DB-free) seed matcher: for each kernel resource_track with a valid id,
/// look up the character's derived value in `resources` (flat `{id: number}`,
/// matched case-insensitively). When found, both current and max are that
/// derived value. When absent, fall back to the track's static kernel
/// `initial`/`max` (fail-closed; never fabricates). Keyed by kernel track id.
pub fn match_seed(resources: &serde_json::Value, kernel_tracks: &[serde_json::Value]) -> std::collections::HashMap<String, (Option<i32>, Option<i32>)> {
    let lookup = |id: &str| -> Option<i32> {
        let want = id.trim().to_ascii_lowercase();
        resources.as_object().and_then(|m| m.iter()
            .find(|(k, _)| k.trim().to_ascii_lowercase() == want)
            .and_then(|(_, v)| v.as_i64().map(|n| n as i32)))
    };
    let mut out = std::collections::HashMap::new();
    for t in kernel_tracks {
        let Some(id) = t.get("id").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { continue };
        let kmax = t.get("max").and_then(|v| v.as_i64()).map(|n| n as i32);
        let kinit = t.get("initial").and_then(|v| v.as_i64()).map(|n| n as i32);
        let derived = lookup(&id);
        out.insert(id, (derived.or(kinit), derived.or(kmax)));  // (current, max)
    }
    out
}

/// Drop malformed resource_tracks (fail-closed): a real track must have a
/// non-empty string `id` OR `name`. Filters out character-sheet field-defs that
/// were mis-submitted as tracks (e.g. `{field_id, field_type, title}`). Generic
/// — no per-ruleset logic. Used at parse-write and kernel-load time.
pub fn normalize_resource_tracks(tracks: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let has = |t: &serde_json::Value, k: &str| t.get(k).and_then(|v| v.as_str()).map(|s| !s.trim().is_empty()).unwrap_or(false);
    tracks.iter().filter(|t| has(t, "id") || has(t, "name")).cloned().collect()
}

/// Map an effect-roll `parameter_path` to a kernel resource-track id (semantic,
/// no hardcoded alias table). `"hp"`/`"hp.current"` resolve via the kernel HP
/// track; `"resources.{X}.current"` (or a bare resource name) matches a track id
/// case-insensitively. Returns None when nothing matches (fail-closed).
pub fn resolve_resource_track_id(parameter_path: &str, kernel: &RuleKernel) -> Option<String> {
    let p = parameter_path.trim();
    let head = p.split('.').next().unwrap_or(p).to_ascii_lowercase();
    if head == "hp" {
        return hp_resource_track_id(&kernel.resource_tracks);
    }
    let candidate = p.strip_prefix("resources.").map(|rest| rest.split('.').next().unwrap_or(rest)).unwrap_or(&head).to_ascii_lowercase();
    kernel.resource_tracks.iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .find(|id| id.trim().to_ascii_lowercase() == candidate)
        .map(|s| s.trim().to_string())
}

/// Pure damage math for an HP effect. Returns `(to_value, sp_applied)`.
/// `Subtract` (and any non-Add/Set op) is armor-mitigated: effective =
/// max(amount - sp, 0), result floored at 0. `Add` heals (sp ignored). `Set`
/// sets the value directly (clamped >= 0). Mirrors the prior inline logic.
pub fn apply_armor_damage(from: i32, amount: i32, armor: Option<i32>, op: ParameterOperation) -> (i32, i32) {
    let sp = armor.unwrap_or(0).max(0);
    match op {
        ParameterOperation::Add => ((from + amount).max(0), 0),
        ParameterOperation::Set => (amount.max(0), 0),
        _ => {
            let eff = (amount - sp).max(0);
            ((from - eff).max(0), sp)
        }
    }
}

/// Derived wound label from a current value vs a max (None when no row/cap).
pub fn wound_label(to: i32, max: Option<i32>) -> &'static str {
    if to <= 0 { return "defeated"; }
    match max { Some(m) if m > 0 && to <= m / 2 => "wounded", _ => "unhurt" }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleKernelPatch {
    pub patch_id: String,
    pub ruleset_id: String,
    pub target_kernel_version: String,
    pub patch_kind: String,
    pub old_hash: String,
    pub proposed_hash: String,
    pub diff_summary: String,
    #[serde(default)]
    pub json_patch: serde_json::Value,
    #[serde(default)]
    pub source_refs: Vec<SourceRef>,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub contradiction_report: Option<serde_json::Value>,
    #[serde(default)]
    pub regression_tests: Vec<serde_json::Value>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleAgentRun {
    pub run_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
    pub trigger: String,
    pub selected_skill: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub source_refs_read: Vec<SourceRef>,
    #[serde(default)]
    pub outputs_written: Vec<String>,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub unresolved_count: usize,
    #[serde(default)]
    pub contradiction_count: usize,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub query: String,
    #[serde(default)]
    pub hit_count: usize,
    #[serde(default)]
    pub selected_hit_ids: Vec<String>,
    #[serde(default)]
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayabilityGateReport {
    pub report_id: String,
    pub ruleset_id: String,
    #[serde(default)]
    pub module_id: Option<String>,
    pub has_rule_kernel: bool,
    pub has_character_sheet_template: bool,
    pub has_character_creation_flow: bool,
    pub has_derived_formula_pack: bool,
    pub has_starter_character_path: bool,
    pub has_first_session_packet: bool,
    #[serde(default)]
    pub blocking_gaps: Vec<PlayabilityGap>,
    #[serde(default)]
    pub warnings: Vec<PlayabilityWarning>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayabilityGap {
    pub gap_id: String,
    pub message: String,
    #[serde(default)]
    pub repair_skill: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayabilityWarning { pub code: String, pub message: String }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterSheet {
    pub character_id: String,
    pub ruleset_id: String,
    pub template_id: String,
    pub name: String,
    pub sheet: serde_json::Value,
    pub validation_report: ValidationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ScenarioNode {
    pub node_id: String,
    pub title: String,
    pub node_type: String,
    pub summary: String,
    pub read_aloud: Option<String>,
    pub gm_notes: Option<String>,
    pub links: Vec<ScenarioLink>,
    pub assets: Vec<String>,
    pub data: serde_json::Value,
    #[serde(default)] pub extraction_status: SceneExtractionStatus,
    #[serde(default)] pub page_start: Option<u32>,
    #[serde(default)] pub page_end: Option<u32>,
    #[serde(default)] pub referenced_npc_ids: Vec<String>,
    #[serde(default)] pub referenced_clue_ids: Vec<String>,
    #[serde(default)] pub referenced_location_ids: Vec<String>,
    #[serde(default)] pub referenced_encounter_ids: Vec<String>,
    #[serde(default)] pub scene_mechanics: Vec<SceneMechanicIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ScenarioLink {
    pub to_node_id: String,
    pub reason: String,
    pub clue_id: Option<String>,
    #[serde(default)] pub link_type: LinkType,
    #[serde(default)] pub source_anchor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ModuleGraph {
    pub module_id: String,
    pub ruleset_id: Option<String>,
    pub title: String,
    pub module_type: DocumentType,
    pub spine: serde_json::Value,
    pub chapters: Vec<ScenarioNode>,
    pub missions: Vec<ScenarioNode>,
    pub scenes: Vec<ScenarioNode>,
    pub locations: Vec<serde_json::Value>,
    pub npcs: Vec<serde_json::Value>,
    pub factions: Vec<serde_json::Value>,
    pub clues: Vec<serde_json::Value>,
    pub handouts: Vec<serde_json::Value>,
    pub encounters: Vec<serde_json::Value>,
    pub module_specific_rules: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ValidationReport {
    pub status: String,
    pub errors: Vec<ValidationMessage>,
    pub warnings: Vec<ValidationMessage>,
    pub info: Vec<ValidationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ValidationMessage {
    pub code: String,
    pub message: String,
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConversionTraceEvent {
    pub at: DateTime<Utc>,
    pub event_type: String,
    pub message: String,
    pub data: serde_json::Value,
}

impl ConversionTraceEvent {
    pub fn new(event_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self { at: Utc::now(), event_type: event_type.into(), message: message.into(), data: serde_json::Value::Null }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuleBundle {
    pub schema_version: String,
    pub bundle_id: String,
    pub ruleset_id: String,
    pub title: String,
    pub source_index: SourceIndex,
    pub parsed_ruleset: ParsedRuleset,
    pub procedure_registry: ProcedureRegistry,
    #[serde(default)]
    pub rule_kernel: Option<RuleKernel>,
    pub character_templates: Vec<CharacterTemplate>,
    #[serde(default)]
    pub character_onboarding_packs: Vec<CharacterOnboardingPack>,
    #[serde(default)]
    pub gm_onboarding: Option<GmOnboardingBundle>,
    pub material_index: Vec<MaterialIndexEntry>,
    pub context_blocks: Vec<ContextBlock>,
    pub validation_report: ValidationReport,
    pub conversion_trace: Vec<ConversionTraceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModuleBundle {
    pub schema_version: String,
    pub bundle_id: String,
    pub module_id: String,
    pub ruleset_id: Option<String>,
    pub title: String,
    pub source_index: SourceIndex,
    pub module_graph: ModuleGraph,
    #[serde(default)]
    pub module_prep_packets: Vec<ModulePrepPacket>,
    #[serde(default)]
    pub module_locators: Vec<BookLocatorEntry>,
    pub material_index: Vec<MaterialIndexEntry>,
    pub context_blocks: Vec<ContextBlock>,
    pub validation_report: ValidationReport,
    pub conversion_trace: Vec<ConversionTraceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProjectBundle {
    pub schema_version: String,
    pub project_id: String,
    pub generated_at: DateTime<Utc>,
    pub source_documents: Vec<SourceDocumentRef>,
    pub rulesets: Vec<RuleBundle>,
    pub modules: Vec<ModuleBundle>,
    pub global_material_index: Vec<MaterialIndexEntry>,
    pub global_context_blocks: Vec<ContextBlock>,
    pub validation_report: ValidationReport,
    pub conversion_trace: Vec<ConversionTraceEvent>,
}

impl ProjectBundle {
    pub fn empty(project_id: impl Into<String>) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION.to_string(),
            project_id: project_id.into(),
            generated_at: Utc::now(),
            source_documents: vec![],
            rulesets: vec![],
            modules: vec![],
            global_material_index: vec![],
            global_context_blocks: vec![],
            validation_report: ValidationReport { status: "ok".to_string(), ..Default::default() },
            conversion_trace: vec![],
        }
    }
}



/// 场景节点的抽取深度态，由 module_reader 两遍式产出。
///
/// - `SkeletonOnly`：廉价骨架态，只填 title/page/refs（页码、引用 npc/物品 id），
///   不含正文；首遍快速扫全书时产生。
/// - `DeepExtracted`：深抽态，已填 read_aloud/gm_notes 等正文字段；
///   background job 续抽时就地把骨架升级为深抽。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SceneExtractionStatus {
    SkeletonOnly,
    DeepExtracted,
}

impl Default for SceneExtractionStatus {
    fn default() -> Self { Self::SkeletonOnly }
}

/// 模组内容分类，决定该内容落到哪个 BP 缓存区。
///
/// - `Bp2CustomRule`：模组自定义规则 → 进 BP2（PinnedMiddle，持久常驻区）。
/// - `Bp3Index`：物品 / 怪物 / 实体索引 → 进 BP3（DynamicTail，按需区）。
/// - `Story`：剧情正文 → 进场景节点（scenario node）。
///
/// 该枚举的 snake_case 线格式是 reader 产出 / parse_module 读取的契约值，
/// 将在 Phase 2/3 被消费。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModuleContentClass {
    Bp2CustomRule,
    Bp3Index,
    Story,
}

impl Default for ModuleContentClass {
    fn default() -> Self { Self::Story }
}

/// 场景节点之间连边的类型：`Spatial`=物理可达、`Trigger`=剧情触发、
/// `Timeline`=时间线推进、`Sequential`=顺序、`Branch`=分支。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    Spatial,
    Trigger,
    Timeline,
    Sequential,
    Branch,
}

impl Default for LinkType {
    fn default() -> Self { Self::Sequential }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningStage {
    Unseen,
    Located,
    LookedUp,
    UsedOnce,
    Stable,
    Memorized,
}

impl Default for LearningStage {
    fn default() -> Self { Self::Unseen }
}

impl LearningStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            LearningStage::Unseen => "unseen",
            LearningStage::Located => "located",
            LearningStage::LookedUp => "looked_up",
            LearningStage::UsedOnce => "used_once",
            LearningStage::Stable => "stable",
            LearningStage::Memorized => "memorized",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RulingConfidence {
    High,
    Medium,
    Low,
}

impl Default for RulingConfidence {
    fn default() -> Self { Self::Medium }
}

impl RulingConfidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            RulingConfidence::High => "high",
            RulingConfidence::Medium => "medium",
            RulingConfidence::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RulingStatus {
    SourceBacked,
    Learned,
    Provisional,
    TableRuling,
    /// A mechanical contract was intentionally not rollable because required
    /// source-backed parameters are missing.  This prevents silent fallback to
    /// invented DV/stat/damage values.
    BlockedMissingParam,
}

impl Default for RulingStatus {
    fn default() -> Self { Self::Provisional }
}

impl RulingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RulingStatus::SourceBacked => "source_backed",
            RulingStatus::Learned => "learned",
            RulingStatus::Provisional => "provisional",
            RulingStatus::TableRuling => "table_ruling",
            RulingStatus::BlockedMissingParam => "blocked_missing_param",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GameIdentity {
    pub genre: String,
    pub tone: String,
    pub player_fantasy: String,
    pub gm_role: String,
    pub failure_style: String,
    pub safety_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayLoop {
    pub default_loop: Vec<String>,
    pub scene_loop: Vec<String>,
    pub combat_loop: Vec<String>,
    pub investigation_loop: Vec<String>,
    pub downtime_loop: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RulesetKernel {
    pub dice_core: String,
    pub when_to_roll: String,
    pub result_bands: Vec<String>,
    pub action_taxonomy: Vec<String>,
    pub consequence_model: String,
    pub starter_procedure_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterSheetMap {
    pub fields: Vec<CharacterSheetMapField>,
    pub common_during_play_fields: Vec<String>,
    pub volatile_fields: Vec<String>,
    pub creation_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CharacterSheetMapField {
    pub field_id: String,
    pub title: String,
    pub meaning: String,
    pub changes_during_play: bool,
    pub used_for: Vec<String>,
    pub lookup_terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BookLocatorEntry {
    pub locator_id: String,
    pub owner_id: String,
    pub owner_kind: String,
    pub label: String,
    pub category: String,
    pub source_document_id: String,
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    pub heading_path: Vec<String>,
    pub search_terms: Vec<String>,
    pub summary: String,
    pub confidence: f32,
    pub parse_policy: String,
    pub tags: Vec<String>,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct LookupRecipe {
    pub recipe_id: String,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub demand: String,
    pub rg_terms: Vec<String>,
    pub scope: serde_json::Value,
    pub expected_sources: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GmOnboardingBundle {
    pub schema_version: String,
    pub onboarding_id: String,
    pub ruleset_id: String,
    pub title: String,
    pub source_refs: Vec<SourceRef>,
    pub game_identity: serde_json::Value,
    pub play_loop: serde_json::Value,
    pub ruleset_kernel: serde_json::Value,
    pub character_sheet_map: serde_json::Value,
    pub book_locator: Vec<BookLocatorEntry>,
    pub starter_procedures: Vec<ProcedureDef>,
    pub lookup_recipes: Vec<LookupRecipe>,
    pub cold_data_locator: Vec<BookLocatorEntry>,
    pub learned_packets_seed: Vec<LearnedPacket>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModulePrepPacket {
    pub prep_id: String,
    pub module_id: String,
    pub ruleset_id: Option<String>,
    pub mode: String,
    pub title: String,
    pub module_overview: serde_json::Value,
    pub current_session_packet: serde_json::Value,
    pub required_rule_demands: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub created_at: DateTime<Utc>,
}


pub type CurrentSessionPacketData = ModulePrepPacket;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LookupEvent {
    pub event_id: String,
    pub session_id: Option<String>,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub demand_id: Option<String>,
    pub query_text: String,
    pub search_terms: Vec<String>,
    pub source_hits: serde_json::Value,
    pub result_status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RulingLogEntry {
    pub ruling_id: String,
    pub session_id: Option<String>,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub demand_id: Option<String>,
    pub ruling_text: String,
    pub source_refs: Vec<SourceRef>,
    pub status: RulingStatus,
    pub confidence: RulingConfidence,
    pub provisional: bool,
    pub superseded_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub type RulingLog = RulingLogEntry;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LearnedPacket {
    pub packet_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub packet_type: String,
    pub packet_key: String,
    pub title: String,
    pub summary: String,
    pub packet_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub use_count: i32,
    pub learning_stage: LearningStage,
    pub confidence: RulingConfidence,
    pub cache_zone: CacheZone,
    pub visibility: Visibility,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LearnedPacket {
    pub fn to_context_block(&self) -> ContextBlock {
        let mut block = ContextBlock::new(
            format!("learned_packet.{}", self.packet_id),
            BlockKind::LearnedPacket,
            &self.title,
            BlockContent::Markdown(self.summary.clone()),
            self.visibility,
            match self.learning_stage {
                LearningStage::Memorized => Stability::RarelyChanged,
                LearningStage::Stable => Stability::SceneStable,
                _ => Stability::TurnDynamic,
            },
            self.cache_zone,
            Scope::ruleset(&self.ruleset_id),
            82,
        );
        block.tags = vec!["learned_packet".into(), self.packet_type.clone(), self.packet_key.clone()];
        block.source_refs = self.source_refs.clone();
        block.load_reason = Some("learned_packet_relevant".into());
        block
    }
}



#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LearningCandidateStatus {
    PendingReview,
    Approved,
    Rejected,
    AutoPromoted,
}

impl Default for LearningCandidateStatus {
    fn default() -> Self { Self::PendingReview }
}

impl LearningCandidateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            LearningCandidateStatus::PendingReview => "pending_review",
            LearningCandidateStatus::Approved => "approved",
            LearningCandidateStatus::Rejected => "rejected",
            LearningCandidateStatus::AutoPromoted => "auto_promoted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LearningAuditCandidate {
    pub candidate_id: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub demand_id: Option<String>,
    pub packet_type: String,
    pub packet_key: String,
    pub title: String,
    pub summary: String,
    pub packet_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub evidence_score: f32,
    pub risk_flags: Vec<String>,
    pub verifier_status: LearningCandidateStatus,
    pub verifier_notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LearningAuditCandidate {
    pub fn to_learned_packet(&self, stage: LearningStage) -> LearnedPacket {
        let now = Utc::now();
        LearnedPacket {
            packet_id: format!("learned_{}", self.candidate_id),
            ruleset_id: self.ruleset_id.clone(),
            module_id: self.module_id.clone(),
            packet_type: self.packet_type.clone(),
            packet_key: self.packet_key.clone(),
            title: self.title.clone(),
            summary: self.summary.clone(),
            packet_json: self.packet_json.clone(),
            source_refs: self.source_refs.clone(),
            use_count: 1,
            learning_stage: stage,
            confidence: if self.evidence_score >= 0.80 { RulingConfidence::High } else if self.evidence_score >= 0.55 { RulingConfidence::Medium } else { RulingConfidence::Low },
            cache_zone: CacheZone::PinnedMiddle,
            visibility: Visibility::GmOnly,
            last_used_at: Some(now),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct LearningAuditResult {
    pub audit_run_id: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub rulings: Vec<RulingLogEntry>,
    pub candidates: Vec<LearningAuditCandidate>,
    pub promoted_packets: Vec<LearnedPacket>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Event,
    Fact,
    Snapshot,
    Relationship,
    ClueState,
    OpenThread,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Superseded,
    Contradicted,
    Archived,
}

impl MemoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryStatus::Active => "active",
            MemoryStatus::Superseded => "superseded",
            MemoryStatus::Contradicted => "contradicted",
            MemoryStatus::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryEvent {
    pub event_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub scene_id: Option<String>,
    pub location_id: Option<String>,
    pub actor_ids: Vec<String>,
    pub visibility: Visibility,
    pub event_kind: MemoryKind,
    pub summary: String,
    pub transcript_excerpt: Option<String>,
    pub source: serde_json::Value,
    pub tags: Vec<String>,
    pub importance: i32,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryFact {
    pub fact_id: String,
    pub session_id: String,
    pub scope: Scope,
    pub visibility: Visibility,
    pub subject: String,
    pub predicate: String,
    pub object: serde_json::Value,
    pub summary: String,
    pub status: MemoryStatus,
    pub confidence: f32,
    pub source_event_ids: Vec<String>,
    pub tags: Vec<String>,
    pub importance: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorySnapshot {
    pub snapshot_id: String,
    pub session_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub scope: Scope,
    pub visibility: Visibility,
    pub title: String,
    pub summary_markdown: String,
    pub included_event_ids: Vec<String>,
    pub included_fact_ids: Vec<String>,
    pub version: u32,
    pub token_estimate: Option<u32>,
    pub content_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemorySnapshot {
    pub fn new(
        snapshot_id: impl Into<String>,
        session_id: impl Into<String>,
        ruleset_id: impl Into<String>,
        module_id: Option<String>,
        scope: Scope,
        visibility: Visibility,
        title: impl Into<String>,
        summary_markdown: impl Into<String>,
        included_event_ids: Vec<String>,
        included_fact_ids: Vec<String>,
        version: u32,
    ) -> Self {
        let summary_markdown = summary_markdown.into();
        let content_hash = sha256_hex(&summary_markdown);
        let token_estimate = Some((summary_markdown.chars().count() as u32 / 4).max(1));
        let now = Utc::now();
        Self {
            snapshot_id: snapshot_id.into(),
            session_id: session_id.into(),
            ruleset_id: ruleset_id.into(),
            module_id,
            scope,
            visibility,
            title: title.into(),
            summary_markdown,
            included_event_ids,
            included_fact_ids,
            version,
            token_estimate,
            content_hash,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryQuery {
    pub session_id: String,
    pub text: String,
    pub ruleset_id: Option<String>,
    pub module_id: Option<String>,
    pub scene_id: Option<String>,
    pub location_id: Option<String>,
    pub actor_ids: Vec<String>,
    pub tags: Vec<String>,
    pub limit: u32,
    pub viewer: VisibilityProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MemoryRetrievalResult {
    pub snapshots: Vec<MemorySnapshot>,
    pub facts: Vec<MemoryFact>,
    pub events: Vec<MemoryEvent>,
    pub blocks: Vec<ContextBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct WorldState {
    pub state: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StatePatch {
    SetTrack { target: String, value: i32, reason: String },
    ModifyTrack { target: String, amount: i32, reason: String },
    ActorHpDelta { actor_id: String, from: Option<i32>, delta: i32, to: Option<i32>, reason: String },
    CreateAspect { target: String, value: serde_json::Value, reason: String },
    RemoveAspect { target: String, reason: String },
    ApplyStatus { target: String, status: serde_json::Value, reason: String },
    RemoveStatus { target: String, reason: String },
    CreateFact { target: String, fact: serde_json::Value, reason: String },
    SetTrait { actor_id: String, trait_id: String, value: serde_json::Value, reason: String },
    GrantObject { actor_id: String, object_id: String, reason: String },
    ObjectPatch { patch_id: String, object_id: Option<String>, patch_json: serde_json::Value, reason: String },
    LoadMaterial { material_id: String, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorResponse {
    pub narration_public: String,
    pub narration_gm: Option<String>,
    pub proposed_patches: Vec<StatePatch>,
    pub follow_up_questions: Vec<String>,
    pub safety_flags: Vec<String>,
}


// -----------------------------------------------------------------------------
// Agentic checks, roll visibility, and lightweight combat contracts (v0.9)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    PlayerCharacter,
    Npc,
    Environment,
    Hazard,
    System,
}

impl Default for ActorKind {
    fn default() -> Self { Self::PlayerCharacter }
}

impl ActorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActorKind::PlayerCharacter => "player_character",
            ActorKind::Npc => "npc",
            ActorKind::Environment => "environment",
            ActorKind::Hazard => "hazard",
            ActorKind::System => "system",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActorRef {
    pub actor_id: String,
    pub actor_kind: ActorKind,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RollVisibility {
    PlayerRollRequired,
    PublicGmRoll,
    PrivateGmRoll,
    PassiveResolution,
    NoRoll,
}

impl Default for RollVisibility {
    fn default() -> Self { Self::NoRoll }
}

impl RollVisibility {
    pub fn as_str(&self) -> &'static str {
        match self {
            RollVisibility::PlayerRollRequired => "player_roll_required",
            RollVisibility::PublicGmRoll => "public_gm_roll",
            RollVisibility::PrivateGmRoll => "private_gm_roll",
            RollVisibility::PassiveResolution => "passive_resolution",
            RollVisibility::NoRoll => "no_roll",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RollAuthority {
    Player,
    GmAgent,
    System,
}

impl Default for RollAuthority {
    fn default() -> Self { Self::GmAgent }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RollDisclosurePolicy {
    pub show_roll_to_player: bool,
    pub show_formula_to_player: bool,
    pub show_dc_to_player: bool,
    pub show_success_failure_to_player: bool,
    pub reveal_after_scene: bool,
    pub reveal_after_session: bool,
}

impl Default for RollDisclosurePolicy {
    fn default() -> Self {
        Self { show_roll_to_player: false, show_formula_to_player: false, show_dc_to_player: false, show_success_failure_to_player: true, reveal_after_scene: false, reveal_after_session: false }
    }
}

impl RollDisclosurePolicy {
    pub fn for_visibility(visibility: RollVisibility) -> Self {
        match visibility {
            RollVisibility::PlayerRollRequired => Self { show_roll_to_player: true, show_formula_to_player: true, show_dc_to_player: true, show_success_failure_to_player: true, reveal_after_scene: false, reveal_after_session: false },
            RollVisibility::PublicGmRoll => Self { show_roll_to_player: true, show_formula_to_player: true, show_dc_to_player: true, show_success_failure_to_player: true, reveal_after_scene: false, reveal_after_session: false },
            RollVisibility::PrivateGmRoll => Self { show_roll_to_player: false, show_formula_to_player: false, show_dc_to_player: false, show_success_failure_to_player: false, reveal_after_scene: true, reveal_after_session: true },
            RollVisibility::PassiveResolution => Self { show_roll_to_player: false, show_formula_to_player: false, show_dc_to_player: false, show_success_failure_to_player: false, reveal_after_scene: false, reveal_after_session: true },
            RollVisibility::NoRoll => Self::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OppositionModel {
    StaticDc { dc: i32, label: String },
    OpposedActive { defender: ActorRef, defender_check_label: String, defender_dice_expression: String },
    PassiveDefense { defender: ActorRef, passive_score_label: String, passive_score: i32 },
    ArmorOrResistance { defender: ActorRef, field_refs: Vec<String> },
    NoMechanicalOpposition,
}

impl Default for OppositionModel {
    fn default() -> Self { Self::NoMechanicalOpposition }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckTargetModel {
    StaticNumber { value: i32, label: String },
    Opposed { opponent_id: String, opponent_check: String },
    DegreeOnly,
    SuccessCount { threshold: i32 },
    /// Dice-pool success counting: roll the pool, count dice showing exactly
    /// `target_face`, succeed when that count >= `threshold` (e.g. Triangle
    /// Agency 6d4, count 3s, succeed on >= 1).
    DicePoolCount { target_face: i32, threshold: i32, label: String },
    UnknownUntilLookup,
}

impl Default for CheckTargetModel {
    fn default() -> Self { Self::UnknownUntilLookup }
}

/// Build a CheckTargetModel from a parsed rule kernel's `dice_core` JSON (the
/// typed `compare` operator + numbers). Returns None when the kernel doesn't
/// specify enough to resolve — callers then keep UnknownUntilLookup and let the
/// contest kernel / operator overrides decide. Shared by the ability, object,
/// and combat check builders so the success rule is data-driven everywhere.
/// Recover a count_faces success face from a kernel `dice_core`: prefer the
/// MACHINE field `target_face`, else parse it from the prose-ish `compare_to`
/// ("3"). The reader sometimes leaves the machine field null but records the
/// face in compare_to (Triangle Agency). Returns None when neither yields an
/// integer — callers then stay fail-closed (never invent a face).
pub fn count_faces_target_face(dice_core: &serde_json::Value) -> Option<i64> {
    dice_core.get("target_face").and_then(|v| v.as_i64()).or_else(|| {
        dice_core.get("compare_to").and_then(|v| v.as_str()).and_then(|s| s.trim().parse::<i64>().ok())
    })
}

/// count_faces success threshold: the MACHINE field `success_threshold`, else
/// the standard "≥1 success face" default that pool games (Triangle) use.
pub fn count_faces_threshold(dice_core: &serde_json::Value) -> i64 {
    dice_core.get("success_threshold").and_then(|v| v.as_i64()).unwrap_or(1)
}

pub fn target_model_from_dice_core(dice_core: &serde_json::Value) -> Option<CheckTargetModel> {
    let compare = dice_core.get("compare").and_then(|v| v.as_str());
    let direction = dice_core.get("direction").and_then(|v| v.as_str()).unwrap_or("");
    let target_number = dice_core.get("target_number").and_then(|v| v.as_i64());
    if compare == Some("count_faces") || direction == "pool_count" {
        // fail-soft via the shared recovery helpers: Triangle's kernel leaves the
        // machine fields null but carries the face in compare_to and "≥1 success"
        // in the prose. Data-driven (no per-ruleset names); fail-closed (None) only
        // when compare_to has no parseable integer face either.
        if let Some(tf) = count_faces_target_face(dice_core) {
            return Some(CheckTargetModel::DicePoolCount {
                target_face: tf as i32,
                threshold: count_faces_threshold(dice_core) as i32,
                label: "kernel core mechanic (dice pool)".into(),
            });
        }
        return None;
    }
    if compare == Some("meet_or_beat") {
        if let Some(tn) = target_number {
            return Some(CheckTargetModel::StaticNumber { value: tn as i32, label: "kernel core mechanic target".into() });
        }
    }
    None
}

#[cfg(test)]
mod target_model_dice_pool_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn count_faces_recovers_target_face_from_compare_to() {
        // Triangle Agency's real kernel: target_face/success_threshold null, but the
        // face ("3") is in compare_to. Before the fix this returned None (check
        // never resolved); now it recovers a DicePoolCount{face=3, threshold=1}.
        let dc = json!({"dice":"6d4","compare":"count_faces","compare_to":"3","target_face":null,"success_threshold":null});
        match target_model_from_dice_core(&dc) {
            Some(CheckTargetModel::DicePoolCount { target_face, threshold, .. }) => {
                assert_eq!(target_face, 3, "face recovered from compare_to");
                assert_eq!(threshold, 1, "count_faces threshold defaults to >=1 success");
            }
            other => panic!("expected DicePoolCount, got {other:?}"),
        }
    }

    #[test]
    fn count_faces_prefers_explicit_machine_fields() {
        let dc = json!({"compare":"count_faces","compare_to":"3","target_face":5,"success_threshold":2});
        match target_model_from_dice_core(&dc) {
            Some(CheckTargetModel::DicePoolCount { target_face, threshold, .. }) => {
                assert_eq!((target_face, threshold), (5, 2), "explicit machine fields win over compare_to");
            }
            other => panic!("expected DicePoolCount, got {other:?}"),
        }
    }

    #[test]
    fn count_faces_unparseable_compare_to_stays_fail_closed() {
        let dc = json!({"compare":"count_faces","compare_to":"the highest die","target_face":null});
        assert!(target_model_from_dice_core(&dc).is_none(), "no parseable face -> None (fail-closed, never invent)");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParamDomain {
    /// A characteristic / attribute (e.g. CoC DEX, POW) under mechanical_profile.stats.
    Stat,
    /// A trained skill (e.g. CoC "Spot Hidden") under mechanical_profile.skills.
    Skill,
    /// A live resource track (e.g. Sanity, Luck) whose current value lives in
    /// generic_parameter_states at `resources.{id}.current`.
    ResourceTrack,
}

/// Which actor parameter a check tests, carried on the contract so the contest
/// resolver reads the actor's REAL value (not a global default). `domain` is an
/// optional hint; when absent the resolver derives the source from the kernel's
/// resource_tracks + the actor's stat/skill keys. `key` is the parameter name as
/// referenced by the action/ability (canonicalized at resolve time).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TestedParameter {
    #[serde(default)]
    pub domain: Option<ParamDomain>,
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CheckModifier {
    pub label: String,
    pub value: i32,
    pub source_ref: Option<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CheckStakes {
    pub before_roll_public: String,
    pub success_public: String,
    pub failure_public: String,
    pub critical_public: Option<String>,
    pub fumble_public: Option<String>,
    pub success_patches_allowed: Vec<String>,
    pub failure_patches_allowed: Vec<String>,
    pub irreversible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckContract {
    pub check_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub initiator: ActorRef,
    pub target_actor: Option<ActorRef>,
    pub opposition: OppositionModel,
    pub action_summary: String,
    pub intent_kind: String,
    pub check_label: String,
    pub dice_expression: String,
    pub modifiers: Vec<CheckModifier>,
    pub target: CheckTargetModel,
    /// Which actor parameter this check tests (e.g. CoC Sanity / Spot Hidden).
    /// Lets the contest resolver read the actor's real value for roll-under
    /// (percentile) families instead of a global default.
    #[serde(default)]
    pub tested_parameter: Option<TestedParameter>,
    /// 对抗检定中防御方该测的参数(B2 通道)。攻击方用 `tested_parameter`,
    /// 防御方用此字段;contest 据此读 defender 卡的真值。None = 非对抗 / 未盖章。
    #[serde(default)]
    pub opponent_tested_parameter: Option<TestedParameter>,
    pub actor_snapshot_ids: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub learned_packet_ids: Vec<String>,
    pub roll_visibility: RollVisibility,
    pub roll_authority: RollAuthority,
    pub disclosure: RollDisclosurePolicy,
    pub stakes: CheckStakes,
    pub confidence: RulingConfidence,
    pub ruling_status: RulingStatus,
    pub advice_refs: Vec<String>,
    pub expires_at_turn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InformationLevel {
    Surface,
    Inferred,
    Hidden,
    GmOnly,
}

impl Default for InformationLevel {
    fn default() -> Self { Self::Surface }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FreeReadContract {
    pub free_read_id: String,
    pub action_summary: String,
    pub reason: String,
    pub information_level: InformationLevel,
    pub source_refs: Vec<SourceRef>,
    pub no_state_change: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnPlanKind {
    NarrateOnly,
    AskPlayerRoll,
    GmRollThenNarrate,
    SecretRollThenNarrate,
    PassiveResolution,
    StartOrContinueCombat,
}

impl Default for TurnPlanKind {
    fn default() -> Self { Self::NarrateOnly }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentTurnPlan {
    pub plan_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub kind: TurnPlanKind,
    pub input_summary: String,
    pub reasoning_summary: String,
    pub policy_layers_used: Vec<String>,
    pub advice_refs: Vec<String>,
    pub check: Option<CheckContract>,
    pub free_read: Option<FreeReadContract>,
    pub combat_action: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl AgentTurnPlan {
    pub fn new(session_id: impl Into<String>, turn_id: impl Into<String>, ruleset_id: impl Into<String>, module_id: Option<String>) -> Self {
        Self { plan_id: format!("agent_plan_{}", Uuid::new_v4().simple()), session_id: session_id.into(), turn_id: turn_id.into(), ruleset_id: ruleset_id.into(), module_id, kind: TurnPlanKind::NarrateOnly, input_summary: String::new(), reasoning_summary: String::new(), policy_layers_used: vec![], advice_refs: vec![], check: None, free_read: None, combat_action: None, created_at: Utc::now() }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingCheckStatus {
    Open,
    Resolved,
    Cancelled,
    AbandonedByNewAction,
    Superseded,
    SupersededByFrameClose,
    SupersededBySceneTransition,
    SupersededByNewFrame,
    SupersededByExitContract,
    SupersededByWorldTimeAdvance,
    SupersededByReconcile,
    StaleGeneration,
    Expired,
}

impl Default for PendingCheckStatus {
    fn default() -> Self { Self::Open }
}

impl PendingCheckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PendingCheckStatus::Open => "open",
            PendingCheckStatus::Resolved => "resolved",
            PendingCheckStatus::Cancelled => "cancelled",
            PendingCheckStatus::AbandonedByNewAction => "abandoned_by_new_action",
            PendingCheckStatus::Superseded => "superseded",
            PendingCheckStatus::SupersededByFrameClose => "superseded_by_frame_close",
            PendingCheckStatus::SupersededBySceneTransition => "superseded_by_scene_transition",
            PendingCheckStatus::SupersededByNewFrame => "superseded_by_new_frame",
            PendingCheckStatus::SupersededByExitContract => "superseded_by_exit_contract",
            PendingCheckStatus::SupersededByWorldTimeAdvance => "superseded_by_world_time_advance",
            PendingCheckStatus::SupersededByReconcile => "superseded_by_reconcile",
            PendingCheckStatus::StaleGeneration => "stale_generation",
            PendingCheckStatus::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PendingCheck {
    pub check_id: String,
    pub session_id: String,
    pub expected_input_kind: String,
    pub prompt_public: String,
    pub contract: CheckContract,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: PendingCheckStatus,
    #[serde(default)]
    pub interaction_context_id: Option<String>,
    #[serde(default)]
    pub owner_frame_id: Option<String>,
    #[serde(default)]
    pub gate_id: Option<String>,
    #[serde(default)]
    pub generation: i64,
    #[serde(default)]
    pub superseded_reason: Option<String>,
    #[serde(default)]
    pub closed_at_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    PlayerRollRequired,
    RequiredReactionChoice,
    OptionalReactionWindow,
    ConfirmRiskyAction,
    ChooseActionMode,
    SpendResourceWindow,
    SelectTarget,
    ResolveAmbiguousIntent,
}

impl Default for GateKind {
    fn default() -> Self { Self::PlayerRollRequired }
}

impl GateKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateKind::PlayerRollRequired => "player_roll_required",
            GateKind::RequiredReactionChoice => "required_reaction_choice",
            GateKind::OptionalReactionWindow => "optional_reaction_window",
            GateKind::ConfirmRiskyAction => "confirm_risky_action",
            GateKind::ChooseActionMode => "choose_action_mode",
            GateKind::SpendResourceWindow => "spend_resource_window",
            GateKind::SelectTarget => "select_target",
            GateKind::ResolveAmbiguousIntent => "resolve_ambiguous_intent",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Open,
    Resolved,
    Cancelled,
    AbandonedByNewAction,
    Superseded,
    SupersededByFrameClose,
    SupersededBySceneTransition,
    SupersededByNewFrame,
    SupersededByExitContract,
    SupersededByWorldTimeAdvance,
    SupersededByReconcile,
    StaleGeneration,
    Expired,
}

impl Default for GateStatus {
    fn default() -> Self { Self::Open }
}

impl GateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateStatus::Open => "open",
            GateStatus::Resolved => "resolved",
            GateStatus::Cancelled => "cancelled",
            GateStatus::AbandonedByNewAction => "abandoned_by_new_action",
            GateStatus::Superseded => "superseded",
            GateStatus::SupersededByFrameClose => "superseded_by_frame_close",
            GateStatus::SupersededBySceneTransition => "superseded_by_scene_transition",
            GateStatus::SupersededByNewFrame => "superseded_by_new_frame",
            GateStatus::SupersededByExitContract => "superseded_by_exit_contract",
            GateStatus::SupersededByWorldTimeAdvance => "superseded_by_world_time_advance",
            GateStatus::SupersededByReconcile => "superseded_by_reconcile",
            GateStatus::StaleGeneration => "stale_generation",
            GateStatus::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActionOption {
    pub option_id: String,
    pub label: String,
    pub meaning: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub consequences: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExpectedInput {
    RollResult { check_id: String, accepted_forms: Vec<String> },
    Choice { option_ids: Vec<String> },
    Confirmation { yes_option: String, no_option: String },
    FreeTextWithClassifier { classifier_skill_id: String },
}

impl Default for ExpectedInput {
    fn default() -> Self {
        ExpectedInput::FreeTextWithClassifier { classifier_skill_id: "none".into() }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateFallbackPolicy {
    Reprompt,
    CancelGateAndContinue,
    TreatAsOption,
    RequireExplicitChoice,
    ResolveAsNoReaction,
    AbortCurrentAction,
}

impl Default for GateFallbackPolicy {
    fn default() -> Self { Self::Reprompt }
}

impl GateFallbackPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateFallbackPolicy::Reprompt => "reprompt",
            GateFallbackPolicy::CancelGateAndContinue => "cancel_gate_and_continue",
            GateFallbackPolicy::TreatAsOption => "treat_as_option",
            GateFallbackPolicy::RequireExplicitChoice => "require_explicit_choice",
            GateFallbackPolicy::ResolveAsNoReaction => "resolve_as_no_reaction",
            GateFallbackPolicy::AbortCurrentAction => "abort_current_action",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InteractionGate {
    pub gate_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub gate_kind: GateKind,
    pub status: GateStatus,
    pub prompt_public: String,
    pub prompt_gm: Option<String>,
    pub required: bool,
    pub allowed_options: Vec<ActionOption>,
    pub expected_input: ExpectedInput,
    pub on_unparseable: GateFallbackPolicy,
    pub on_new_action: GateFallbackPolicy,
    pub on_timeout: GateFallbackPolicy,
    pub bound_action_summary: String,
    pub source_refs: Vec<SourceRef>,
    pub advice_refs: Vec<String>,
    pub resolution_json: Option<serde_json::Value>,
    pub expires_at_turn: Option<String>,
    pub expires_at_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub interaction_context_id: Option<String>,
    #[serde(default)]
    pub owner_frame_id: Option<String>,
    #[serde(default)]
    pub generation: i64,
    #[serde(default)]
    pub superseded_reason: Option<String>,
    #[serde(default)]
    pub closed_at_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl InteractionGate {
    pub fn from_pending_check(pending: &PendingCheck) -> Self {
        let now = Utc::now();
        Self {
            gate_id: format!("gate_{}", pending.check_id),
            session_id: pending.session_id.clone(),
            turn_id: pending.contract.turn_id.clone(),
            gate_kind: GateKind::PlayerRollRequired,
            status: match pending.status {
                PendingCheckStatus::Open => GateStatus::Open,
                PendingCheckStatus::Resolved => GateStatus::Resolved,
                PendingCheckStatus::Cancelled => GateStatus::Cancelled,
                PendingCheckStatus::AbandonedByNewAction => GateStatus::AbandonedByNewAction,
                PendingCheckStatus::Superseded => GateStatus::Superseded,
                PendingCheckStatus::SupersededByFrameClose => GateStatus::SupersededByFrameClose,
                PendingCheckStatus::SupersededBySceneTransition => GateStatus::SupersededBySceneTransition,
                PendingCheckStatus::SupersededByNewFrame => GateStatus::SupersededByNewFrame,
                PendingCheckStatus::SupersededByExitContract => GateStatus::SupersededByExitContract,
                PendingCheckStatus::SupersededByWorldTimeAdvance => GateStatus::SupersededByWorldTimeAdvance,
                PendingCheckStatus::SupersededByReconcile => GateStatus::SupersededByReconcile,
                PendingCheckStatus::StaleGeneration => GateStatus::StaleGeneration,
                PendingCheckStatus::Expired => GateStatus::Expired,
            },
            prompt_public: pending.prompt_public.clone(),
            prompt_gm: Some("Resolve this gate before treating later player input as free narration unless the player explicitly abandons the bound action.".into()),
            required: true,
            allowed_options: vec![],
            expected_input: ExpectedInput::RollResult {
                check_id: pending.check_id.clone(),
                accepted_forms: vec!["/roll 1d10+stat+skill".into(), "bare total".into(), "natural language with result number".into()],
            },
            on_unparseable: GateFallbackPolicy::Reprompt,
            on_new_action: GateFallbackPolicy::CancelGateAndContinue,
            on_timeout: GateFallbackPolicy::AbortCurrentAction,
            bound_action_summary: pending.contract.action_summary.clone(),
            source_refs: pending.contract.source_refs.clone(),
            advice_refs: pending.contract.advice_refs.clone(),
            resolution_json: None,
            expires_at_turn: pending.contract.expires_at_turn.clone(),
            expires_at_time: pending.expires_at,
            interaction_context_id: pending.interaction_context_id.clone(),
            owner_frame_id: pending.owner_frame_id.clone(),
            generation: pending.generation,
            superseded_reason: pending.superseded_reason.clone(),
            closed_at_tick: pending.closed_at_tick,
            created_at: pending.created_at,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GateHandlingResult {
    None,
    PendingCheckResolved { result: CheckResultRecord },
    PendingFollowupCheckCreated { result: CheckResultRecord, pending: Box<PendingCheck>, prompt_public: String, reason: String },
    GateReprompt { gate_id: String, check_id: String, reason: String, prompt_public: String },
    GateAbandoned { gate_id: String, check_id: String, reason: String, prompt_public: String },
    GateChoiceResolved { gate_id: String, option_id: String, option_label: String, resolution_json: serde_json::Value },
    GateChoiceReprompt { gate_id: String, reason: String, prompt_public: String },
    GateSuperseded { gate_id: String, reason: String, prompt_public: String },
}

impl Default for GateHandlingResult {
    fn default() -> Self { Self::None }
}


// -----------------------------------------------------------------------------
// Interaction Lifecycle Kernel data contracts (v1.5)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionContextKind { Session, Frame, Scene, Check, Gate, ReactionWindow, Stalemate, ExitContract }
impl Default for InteractionContextKind { fn default() -> Self { Self::Session } }
impl InteractionContextKind { pub fn as_str(&self) -> &'static str { match self { Self::Session => "session", Self::Frame => "frame", Self::Scene => "scene", Self::Check => "check", Self::Gate => "gate", Self::ReactionWindow => "reaction_window", Self::Stalemate => "stalemate", Self::ExitContract => "exit_contract" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionContextStatus { Active, Paused, Resolving, Closing, Closed, Superseded, Abandoned, Repaired }
impl Default for InteractionContextStatus { fn default() -> Self { Self::Active } }
impl InteractionContextStatus { pub fn as_str(&self) -> &'static str { match self { Self::Active => "active", Self::Paused => "paused", Self::Resolving => "resolving", Self::Closing => "closing", Self::Closed => "closed", Self::Superseded => "superseded", Self::Abandoned => "abandoned", Self::Repaired => "repaired" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupersededReason { FrameClose, SceneTransition, NewFrame, ExitContract, WorldTimeAdvance, TerminalIntent, Reconcile, StaleGeneration, UserAbandoned }
impl Default for SupersededReason { fn default() -> Self { Self::Reconcile } }
impl SupersededReason { pub fn as_str(&self) -> &'static str { match self { Self::FrameClose => "superseded_by_frame_close", Self::SceneTransition => "superseded_by_scene_transition", Self::NewFrame => "superseded_by_new_frame", Self::ExitContract => "superseded_by_exit_contract", Self::WorldTimeAdvance => "superseded_by_world_time_advance", Self::TerminalIntent => "superseded_by_terminal_intent", Self::Reconcile => "superseded_by_reconcile", Self::StaleGeneration => "superseded_by_stale_generation", Self::UserAbandoned => "superseded_by_user_abandoned" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct InteractionOwner { pub owner_kind: String, pub owner_id: Option<String>, #[serde(default)] pub owner_json: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InteractionContext {
    pub context_id: String,
    pub session_id: String,
    pub frame_id: Option<String>,
    pub context_kind: InteractionContextKind,
    pub status: InteractionContextStatus,
    pub generation: i64,
    #[serde(default)] pub active_gate_ids: Vec<String>,
    #[serde(default)] pub pending_check_ids: Vec<String>,
    #[serde(default)] pub reaction_window_ids: Vec<String>,
    #[serde(default)] pub object_interaction_ids: Vec<String>,
    #[serde(default)] pub child_context_ids: Vec<String>,
    pub opened_at_tick: i64,
    pub closed_at_tick: Option<i64>,
    pub owner: InteractionOwner,
    #[serde(default)] pub context_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl Default for InteractionContext { fn default() -> Self { Self { context_id: String::new(), session_id: String::new(), frame_id: None, context_kind: InteractionContextKind::Session, status: InteractionContextStatus::Active, generation: 0, active_gate_ids: vec![], pending_check_ids: vec![], reaction_window_ids: vec![], object_interaction_ids: vec![], child_context_ids: vec![], opened_at_tick: 0, closed_at_tick: None, owner: InteractionOwner::default(), context_json: serde_json::Value::Null, created_at: Utc::now(), updated_at: Utc::now() } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEventKind { ReconcileSession, OpenContext, CloseFrameCascade, OpenGate, ResolveGate, RepromptGate, SupersedeGate, CreatePendingCheck, ResolvePendingCheck, SupersedePendingCheck, CreateObjectInteraction, ResolveObjectInteraction, SupersedeObjectInteraction, InvariantRepair, GenerationAdvanced }
impl Default for InteractionEventKind { fn default() -> Self { Self::ReconcileSession } }
impl InteractionEventKind { pub fn as_str(&self) -> &'static str { match self { Self::ReconcileSession => "reconcile_session", Self::OpenContext => "open_context", Self::CloseFrameCascade => "close_frame_cascade", Self::OpenGate => "open_gate", Self::ResolveGate => "resolve_gate", Self::RepromptGate => "reprompt_gate", Self::SupersedeGate => "supersede_gate", Self::CreatePendingCheck => "create_pending_check", Self::ResolvePendingCheck => "resolve_pending_check", Self::SupersedePendingCheck => "supersede_pending_check", Self::CreateObjectInteraction => "create_object_interaction", Self::ResolveObjectInteraction => "resolve_object_interaction", Self::SupersedeObjectInteraction => "supersede_object_interaction", Self::InvariantRepair => "invariant_repair", Self::GenerationAdvanced => "generation_advanced" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InteractionEvent { pub event_id: String, pub session_id: String, pub interaction_context_id: Option<String>, pub frame_id: Option<String>, pub world_tick: i64, pub generation: i64, pub event_kind: InteractionEventKind, #[serde(default)] pub event_json: serde_json::Value, pub created_at: DateTime<Utc> }
impl Default for InteractionEvent { fn default() -> Self { Self { event_id: String::new(), session_id: String::new(), interaction_context_id: None, frame_id: None, world_tick: 0, generation: 0, event_kind: InteractionEventKind::ReconcileSession, event_json: serde_json::Value::Null, created_at: Utc::now() } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvariantRepairKind { OpenGateWithoutActiveOwner, OpenPendingCheckWithoutActiveOwner, OpenPendingCheckWithoutOpenGate, OpenReactionWithoutActiveFrame, FrameClosedChildrenTerminal, StaleGeneration, MultiplePrimaryContexts, SceneEpochMismatch }
impl Default for InvariantRepairKind { fn default() -> Self { Self::OpenGateWithoutActiveOwner } }
impl InvariantRepairKind { pub fn as_str(&self) -> &'static str { match self { Self::OpenGateWithoutActiveOwner => "open_gate_without_active_owner", Self::OpenPendingCheckWithoutActiveOwner => "open_pending_check_without_active_owner", Self::OpenPendingCheckWithoutOpenGate => "open_pending_check_without_open_gate", Self::OpenReactionWithoutActiveFrame => "open_reaction_without_active_frame", Self::FrameClosedChildrenTerminal => "frame_closed_children_terminal", Self::StaleGeneration => "stale_generation", Self::MultiplePrimaryContexts => "multiple_primary_contexts", Self::SceneEpochMismatch => "scene_epoch_mismatch" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvariantRepair { pub repair_id: String, pub kind: InvariantRepairKind, pub target_table: String, pub target_id: String, pub action: String, pub reason: String, pub world_tick: i64, pub generation: i64 }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct InteractionTransitionResult { #[serde(default)] pub events: Vec<InteractionEvent>, #[serde(default)] pub world_events: Vec<WorldEvent>, #[serde(default)] pub repairs: Vec<InvariantRepair>, #[serde(default)] pub sse_events: Vec<serde_json::Value>, #[serde(default)] pub diagnostics: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiceRollRecord {
    pub roll_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub check_id: Option<String>,
    pub roller_kind: ActorKind,
    pub roller_id: Option<String>,
    pub visibility: RollVisibility,
    pub expression: String,
    pub result: serde_json::Value,
    pub seed_commitment: String,
    pub revealed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckResultRecord {
    pub check_id: String,
    pub roll: DiceRollRecord,
    pub outcome: serde_json::Value,
    pub committed_patches: Vec<StatePatch>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolCallRecord {
    pub tool_call_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub tool_name: String,
    pub visibility: Visibility,
    pub input_json: serde_json::Value,
    pub output_json: Option<serde_json::Value>,
    pub status: String,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Combat,
    Chase,
    Netrun,
    InvestigationNode,
    SocialConflict,
    Negotiation,
    Infiltration,
    HazardSequence,
    SideQuest,
    DowntimeProject,
    AnomalyEncounter,
    HorrorEncounter,
    /// 三期姿态框架：幕间/休整姿态的 frame 种类（serde snake_case = "downtime"；
    /// 旧数据无此值 + Default=SideQuest ⇒ 向后兼容）。
    Downtime,
}

impl Default for FrameKind {
    fn default() -> Self { Self::SideQuest }
}

impl FrameKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameKind::Combat => "combat",
            FrameKind::Chase => "chase",
            FrameKind::Netrun => "netrun",
            FrameKind::InvestigationNode => "investigation_node",
            FrameKind::SocialConflict => "social_conflict",
            FrameKind::Negotiation => "negotiation",
            FrameKind::Infiltration => "infiltration",
            FrameKind::HazardSequence => "hazard_sequence",
            FrameKind::SideQuest => "sidequest",
            FrameKind::DowntimeProject => "downtime_project",
            FrameKind::AnomalyEncounter => "anomaly_encounter",
            FrameKind::HorrorEncounter => "horror_encounter",
            FrameKind::Downtime => "downtime",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameStatus {
    Active,
    Paused,
    Resolving,
    Completed,
    Failed,
    Abandoned,
    Compacted,
    Archived,
}

impl Default for FrameStatus {
    fn default() -> Self { Self::Active }
}

impl FrameStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameStatus::Active => "active",
            FrameStatus::Paused => "paused",
            FrameStatus::Resolving => "resolving",
            FrameStatus::Completed => "completed",
            FrameStatus::Failed => "failed",
            FrameStatus::Abandoned => "abandoned",
            FrameStatus::Compacted => "compacted",
            FrameStatus::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FrameFact {
    pub fact_id: String,
    pub summary: String,
    pub visibility: Visibility,
    pub importance: i32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FrameModifier {
    pub modifier_id: String,
    pub target: String,
    pub summary: String,
    pub expires_when: Option<String>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct Clock {
    pub clock_id: String,
    pub title: String,
    pub current: i32,
    pub max: i32,
    pub visibility: Visibility,
    #[serde(default)]
    pub deadline_tick: Option<i64>,
    #[serde(default)]
    pub last_tick_at: Option<i64>,
    #[serde(default)]
    pub tick_policy: Option<String>,
}

// -----------------------------------------------------------------------------
// World Time Spine data contracts (v1.4)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeScale { Instant, CombatRound, SceneBeat, Exploration, Travel, Downtime, Flashback }
impl Default for TimeScale { fn default() -> Self { Self::SceneBeat } }
impl TimeScale { pub fn as_str(&self) -> &'static str { match self { TimeScale::Instant => "instant", TimeScale::CombatRound => "combat_round", TimeScale::SceneBeat => "scene_beat", TimeScale::Exploration => "exploration", TimeScale::Travel => "travel", TimeScale::Downtime => "downtime", TimeScale::Flashback => "flashback" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeMutationKind { Advance, NoAdvance, FlashbackFrame, RetconCompensation, TimelineFork }
impl Default for TimeMutationKind { fn default() -> Self { Self::Advance } }
impl TimeMutationKind { pub fn as_str(&self) -> &'static str { match self { TimeMutationKind::Advance => "advance", TimeMutationKind::NoAdvance => "no_advance", TimeMutationKind::FlashbackFrame => "flashback_frame", TimeMutationKind::RetconCompensation => "retcon_compensation", TimeMutationKind::TimelineFork => "timeline_fork" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TimeAmount {
    #[serde(default)] pub seconds: i64,
    #[serde(default)] pub minutes: i64,
    #[serde(default)] pub hours: i64,
    #[serde(default)] pub days: i64,
    #[serde(default)] pub combat_rounds: i64,
    #[serde(default)] pub scene_beats: i64,
    #[serde(default)] pub label: String,
}
impl TimeAmount {
    pub fn total_seconds(&self) -> i64 { self.seconds + self.minutes * 60 + self.hours * 3600 + self.days * 86400 + self.combat_rounds * 3 + self.scene_beats * 60 }
    pub fn minutes(minutes: i64) -> Self { Self { minutes, label: format!("{minutes} minute(s)"), ..Default::default() } }
    pub fn combat_rounds(rounds: i64) -> Self { Self { combat_rounds: rounds, label: format!("{rounds} combat round(s)"), ..Default::default() } }
    pub fn scene_beats(beats: i64) -> Self { Self { scene_beats: beats, label: format!("{beats} scene beat(s)"), ..Default::default() } }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct WorldTimeState {
    pub campaign_id: String,
    pub session_id: String,
    pub world_tick: i64,
    pub absolute_seconds: i64,
    pub calendar_id: String,
    pub display_time: String,
    pub time_scale: TimeScale,
    pub scene_epoch: Option<String>,
    pub turn_seq: i64,
    pub event_seq: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorldEventKind {
    PlayerAction, NpcAction, CheckCreated, CheckResolved, EffectCreated, EffectApplied,
    ExitContractCreated, StalemateContractCreated, ClockTick, TimeAdvanced, SceneChanged,
    ClueDiscovered, NpcAttitudeChanged, MemoryFactCreated, LearningCandidateCreated,
    TableRuling, FrameOpened, FrameClosed, DirectorBriefCreated, NoveltyFreshChange,
    ScheduledEventDue, RetconCompensation, SystemEvent,
}
impl Default for WorldEventKind { fn default() -> Self { Self::SystemEvent } }
impl WorldEventKind { pub fn as_str(&self) -> &'static str { match self { WorldEventKind::PlayerAction => "player_action", WorldEventKind::NpcAction => "npc_action", WorldEventKind::CheckCreated => "check_created", WorldEventKind::CheckResolved => "check_resolved", WorldEventKind::EffectCreated => "effect_created", WorldEventKind::EffectApplied => "effect_applied", WorldEventKind::ExitContractCreated => "exit_contract_created", WorldEventKind::StalemateContractCreated => "stalemate_contract_created", WorldEventKind::ClockTick => "clock_tick", WorldEventKind::TimeAdvanced => "time_advanced", WorldEventKind::SceneChanged => "scene_changed", WorldEventKind::ClueDiscovered => "clue_discovered", WorldEventKind::NpcAttitudeChanged => "npc_attitude_changed", WorldEventKind::MemoryFactCreated => "memory_fact_created", WorldEventKind::LearningCandidateCreated => "learning_candidate_created", WorldEventKind::TableRuling => "table_ruling", WorldEventKind::FrameOpened => "frame_opened", WorldEventKind::FrameClosed => "frame_closed", WorldEventKind::DirectorBriefCreated => "director_brief_created", WorldEventKind::NoveltyFreshChange => "novelty_fresh_change", WorldEventKind::ScheduledEventDue => "scheduled_event_due", WorldEventKind::RetconCompensation => "retcon_compensation", WorldEventKind::SystemEvent => "system_event" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct WorldEvent {
    pub event_id: String,
    pub campaign_id: String,
    pub session_id: String,
    pub world_tick: i64,
    pub event_seq: i64,
    pub turn_id: Option<String>,
    pub frame_id: Option<String>,
    pub event_kind: WorldEventKind,
    pub event_json: serde_json::Value,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub caused_by_event_ids: Vec<String>,
    pub state_patch_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledEventStatus { Pending, Due, Fired, Cancelled, Superseded }
impl Default for ScheduledEventStatus { fn default() -> Self { Self::Pending } }
impl ScheduledEventStatus { pub fn as_str(&self) -> &'static str { match self { ScheduledEventStatus::Pending => "pending", ScheduledEventStatus::Due => "due", ScheduledEventStatus::Fired => "fired", ScheduledEventStatus::Cancelled => "cancelled", ScheduledEventStatus::Superseded => "superseded" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ScheduledEvent {
    pub scheduled_event_id: String,
    pub campaign_id: String,
    pub session_id: String,
    pub due_tick: i64,
    pub event_kind: WorldEventKind,
    pub payload_json: serde_json::Value,
    pub visibility: Visibility,
    pub status: ScheduledEventStatus,
    pub created_by_event_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TimeAdvanceRequest {
    pub session_id: String,
    #[serde(default)] pub campaign_id: Option<String>,
    pub reason: String,
    pub amount: TimeAmount,
    pub scale: TimeScale,
    #[serde(default)] pub mutation_kind: TimeMutationKind,
    #[serde(default)] pub visibility: Visibility,
    #[serde(default)] pub caused_by_turn_id: Option<String>,
    #[serde(default)] pub caused_by_event_id: Option<String>,
    #[serde(default)] pub scene_epoch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TimeAdvanceResult {
    pub from: WorldTimeState,
    pub to: WorldTimeState,
    pub advance_event: Option<WorldEvent>,
    pub triggered_events: Vec<WorldEvent>,
    pub scheduled_events_due: Vec<ScheduledEvent>,
    pub expired_effect_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ContextWatermark {
    pub session_id: String,
    pub last_compiled_world_tick: i64,
    pub last_compiled_event_seq: i64,
    pub compiled_context_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TimeAnchor {
    pub anchor_id: String,
    pub session_id: String,
    pub world_tick: i64,
    pub event_seq: i64,
    pub label: String,
    pub display_time: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RetentionPolicy {
    pub keep_event_log: bool,
    pub compact_on_completion: bool,
    pub keep_last_events: u32,
    pub promote_facts_with_importance_at_least: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CompactionPolicy {
    pub summary_target: String,
    pub preserve_world_patches: bool,
    pub preserve_npc_impacts: bool,
    pub preserve_player_costs: bool,
    pub preserve_open_hooks: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StateFrame {
    pub frame_id: String,
    pub frame_kind: FrameKind,
    pub session_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub parent_frame_id: Option<String>,
    pub scope: Scope,
    pub status: FrameStatus,
    pub title: String,
    pub objective: String,
    pub static_refs: Vec<String>,
    pub working_state: serde_json::Value,
    pub active_gate_ids: Vec<String>,
    pub local_clocks: Vec<Clock>,
    pub local_facts: Vec<FrameFact>,
    pub local_modifiers: Vec<FrameModifier>,
    pub event_count: u32,
    pub last_event_ids: Vec<String>,
    pub retention_policy: RetentionPolicy,
    pub compaction_policy: CompactionPolicy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl StateFrame {
    pub fn to_context_block(&self, turn_id: &str) -> ContextBlock {
        let mut block = ContextBlock::new(
            format!("state_frame.{}", self.frame_id),
            BlockKind::StateFrame,
            format!("Working State Frame: {}", self.title),
            BlockContent::Json(serde_json::to_value(self).unwrap_or_else(|_| serde_json::Value::Null)),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope { scope_type: ScopeType::Turn, scope_id: turn_id.to_string() },
            125,
        );
        block.tags = vec!["working_state_frame".into(), self.frame_kind.as_str().into(), self.status.as_str().into()];
        block.expires_at_turn = Some(turn_id.to_string());
        block.load_reason = Some("active_working_state_frame".into());
        block
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FrameEvent {
    pub event_id: String,
    pub frame_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub event_kind: String,
    pub event_json: serde_json::Value,
    pub visibility: Visibility,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FrameCompaction {
    pub compaction_id: String,
    pub frame_id: String,
    pub session_id: String,
    pub summary_markdown: String,
    pub persistent_world_patches: Vec<StatePatch>,
    pub promoted_fact_ids: Vec<String>,
    pub archived_event_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatFrame {
    pub combat_id: String,
    pub session_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub mode: String,
    pub status: String,
    pub round: u32,
    pub active_actor_id: Option<String>,
    pub participants: Vec<serde_json::Value>,
    pub zones: Vec<serde_json::Value>,
    pub hazards: Vec<serde_json::Value>,
    pub objectives: Vec<serde_json::Value>,
    pub stable_encounter_packet_ids: Vec<String>,
    pub dynamic_state_refs: Vec<String>,
    pub visibility: Visibility,
}


// -----------------------------------------------------------------------------
// Conflict Frame & Combat Agent data contracts (v1.0)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CombatMode {
    TacticalCombat,
    TheaterOfMind,
    Firefight,
    Chase,
    Netrun,
    AnomalyEncounter,
    HorrorEncounter,
    SocialConflict,
    HazardSequence,
}

impl Default for CombatMode {
    fn default() -> Self { Self::TheaterOfMind }
}

impl CombatMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CombatMode::TacticalCombat => "tactical_combat",
            CombatMode::TheaterOfMind => "theater_of_mind",
            CombatMode::Firefight => "firefight",
            CombatMode::Chase => "chase",
            CombatMode::Netrun => "netrun",
            CombatMode::AnomalyEncounter => "anomaly_encounter",
            CombatMode::HorrorEncounter => "horror_encounter",
            CombatMode::SocialConflict => "social_conflict",
            CombatMode::HazardSequence => "hazard_sequence",
        }
    }
}

// ---------------------------------------------------------------------------
// P0-2 engine-dehardcode: kernel-resident strategy fields + module config.
// All Value-shaped to keep trpg-model dependency-free of engine crates; the
// engine maps these into its typed structs (RulesetCombatProfile etc.).
// ---------------------------------------------------------------------------

/// Combat action-economy/initiative/reaction/search strategy, kernel-resident.
/// Mirrors trpg-combat::RulesetCombatProfile's strategy sub-blocks as Value so
/// the engine maps kernel→its typed profile without a model→combat dependency.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct CombatProfile {
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub default_mode: String,
    #[serde(default)]
    pub applies_to_modes: Vec<String>,
    #[serde(default)]
    pub action_economy: serde_json::Value,
    #[serde(default)]
    pub initiative: serde_json::Value,
    /// Reaction-window advice entries (serialized ReactionAdvice shape).
    #[serde(default)]
    pub reaction_windows: Vec<serde_json::Value>,
    #[serde(default)]
    pub frame_exit_policy: serde_json::Value,
    #[serde(default)]
    pub stalemate_policy: serde_json::Value,
    #[serde(default)]
    pub npc_drive_policy: serde_json::Value,
    #[serde(default)]
    pub search_recipes: Vec<serde_json::Value>,
}

/// One intent→CombatMode rule. `mode` is a CombatMode (snake_case via as_str).
/// `when_action_kinds` (empty = any) AND `when_evidence_contains` (empty = any)
/// gate the rule; first matching rule wins (engine evaluates in order).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct CombatModeRule {
    pub mode: CombatMode,
    #[serde(default)]
    pub when_action_kinds: Vec<String>,
    #[serde(default)]
    pub when_evidence_contains: Vec<String>,
}

/// Ordered intent→CombatMode mapping. `fallback_mode` applies when no rule hits
/// (replaces infer_combat_mode_from_intent's trailing TheaterOfMind).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct CombatModePolicy {
    #[serde(default)]
    pub rules: Vec<CombatModeRule>,
    #[serde(default)]
    pub fallback_mode: CombatMode,
}

/// Ruleset-specific search-section hints and field-alias overrides per SearchSkillKind.
/// Keys are the SearchSkillKind skill_key() short form (e.g. "combat_resolution").
/// Replaces trpg-material's ruleset_aliases_for contains(ruleset_name) branches.
/// Missing key → engine uses its generic base for that skill. None → all generics.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct RuleKernelSearchProfile {
    /// skill_key → preferred_sections array override. Missing kind → base generic.
    #[serde(default)]
    pub preferred_sections_by_skill: std::collections::HashMap<String, Vec<String>>,
    /// skill_key → field_aliases object override (merged onto base).
    #[serde(default)]
    pub field_aliases_by_skill: std::collections::HashMap<String, serde_json::Value>,
}

impl SearchSkillKind {
    /// Short key used in RuleKernelSearchProfile maps (no "_search" suffix).
    pub fn skill_key(&self) -> &'static str {
        match self {
            Self::CombatResolution => "combat_resolution",
            Self::WeaponParameter => "weapon_parameter",
            Self::ArmorDefense => "armor_defense",
            Self::AbilityActivation => "ability_activation",
            Self::ConditionResource => "condition_resource",
            Self::NpcStatblock => "npc_stat_block",
            Self::ModuleCard => "module_card",
            Self::SceneObject => "scene_object",
            Self::GenericMechanical => "generic_mechanical",
        }
    }
}

/// Check-label templates keyed by action family (technical/attack/defense/…).
/// Replaces combat/object contains("cyberpunk") label branches. Missing key →
/// engine uses the generic label.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct CheckLabelPolicy {
    /// action-family-id → label template, e.g. "technical" → "appropriate
    /// TECH / Interface / Basic Tech check".
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

/// Damage/difficulty family + band text + plausibility ranges. Replaces
/// trpg-referee's ruleset_damage_family/common_damage_band/
/// damage_plausible_for_ruleset/common_difficulty_band_json/
/// difficulty_plausible_for_ruleset. Ranges are inclusive (lo, hi).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct RefereeValueBands {
    #[serde(default)]
    pub damage_family: String,
    #[serde(default)]
    pub damage_band: String,
    /// Plausible dice-count range for a damage expression (matches
    /// damage_plausible_for_ruleset's parse_dice_count bounds).
    #[serde(default)]
    pub damage_plausible_range: (i64, i64),
    /// difficulty band advisory (mirrors common_difficulty_band_json's Value).
    #[serde(default)]
    pub difficulty_band: serde_json::Value,
    /// Plausible target-number range (matches difficulty_plausible_for_ruleset).
    #[serde(default)]
    pub difficulty_plausible_range: (i64, i64),
}

/// Bare-dice qualification template — `{dice}` is the bare expr (e.g. "1d10").
/// Replaces combat's hardcoded "1d10+0".
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct DiceQualification {
    /// e.g. "{dice}+0"; engine substitutes the bare dice expr for `{dice}`.
    #[serde(default)]
    pub bare_dice_template: String,
}

// ---------------------------------------------------------------------------
// P0-2 module-level config (lives in the module bundle; #[serde(default)]).
// Replaces target_actor_for_combat_input npc bindings, inferred_homecoming_
// tech_dv, director scene facts, material module_preferences_for.
// ---------------------------------------------------------------------------

/// One keyword/semantic matcher → engine actor_id. Replaces combat's
/// npc.scav_boss / npc.athena_drone literals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct NpcActorBinding {
    /// case-insensitive substrings; any hit binds. (Semantic matcher TBD by
    /// the consuming engine; keyword form is the equivalence baseline.)
    #[serde(default)]
    pub matcher: Vec<String>,
    pub actor_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// One technical-option DV row. Replaces inferred_homecoming_tech_dv's 14/12.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct TechOption {
    #[serde(default)]
    pub matcher: Vec<String>,
    pub dv: i32,
}

/// Scene entity alias (display label ↔ canonical id) for director scene facts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct EntityAlias {
    pub canonical_id: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Module search preferences per SearchSkillKind — replaces material's module_preferences_for
/// homecoming/masks/vault literal lists. Keys are SearchSkillKind::skill_key() short form
/// (e.g. "npc_stat_block"). Missing key → engine uses generic base for that skill.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct SearchProfile {
    /// skill_key → extra preferred section strings (appended to generic base).
    #[serde(default)]
    pub preferred_sections_by_skill: std::collections::HashMap<String, Vec<String>>,
}

/// One module-specific visible scene fact for the director brief
/// (replaces trpg-director's is_homecoming visible_facts hardcode).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct DirectorSceneFact {
    pub text: String,
    #[serde(default)]
    pub source: String,
}

/// One module-specific pressure item (replaces is_homecoming pressure hardcode).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct DirectorPressureItem {
    pub text: String,
    #[serde(default)]
    pub clock_id: Option<String>,
    #[serde(default)]
    pub severity: i32,
    #[serde(default)]
    pub consequence_hint: Option<String>,
}

/// One module-specific affordance / interaction handle (replaces is_homecoming
/// affordances hardcode). `implies_vectors` are ActionVector serde names; empty
/// → engine applies a neutral default set (fail-soft).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct DirectorAffordanceItem {
    pub description: String,
    #[serde(default)]
    pub implies_vectors: Vec<String>,
}

/// One module-specific risk item (replaces is_homecoming risk hardcode).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct DirectorRiskItem {
    pub text: String,
    #[serde(default)]
    pub related_vectors: Vec<String>,
    #[serde(default)]
    pub severity: i32,
}

/// Module-level director facilitation config. Replaces trpg-director's
/// `is_homecoming()` hardcode: scene facts / pressure / affordances / risks /
/// biased NPC advice / open question / no-location place summary now come from
/// this data, branched on DATA presence (not ruleset/module names).
///
/// TRANSITION (DONE_WITH_CONCERNS): module deep data is stored as scene-level
/// `ScenarioNode` prose (read_aloud/gm_notes); there is no module-level
/// container for cross-scene facilitation facts / biased-NPC-advice personas.
/// So these values are injected via this data override file
/// (`{id}.module_config.json`) for now; a future module-reader pass should
/// auto-extract them from parsed scene data and retire the override.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct DirectorModuleConfig {
    /// Module-specific visible scene facts → brief.visible_facts
    #[serde(default)]
    pub scene_facts: Vec<DirectorSceneFact>,
    /// Module-specific pressure items → brief.pressure
    #[serde(default)]
    pub pressure_items: Vec<DirectorPressureItem>,
    /// Module-specific interaction handles → brief.affordances
    #[serde(default)]
    pub affordance_items: Vec<DirectorAffordanceItem>,
    /// Module-specific risks → brief.risks
    #[serde(default)]
    pub risk_items: Vec<DirectorRiskItem>,
    /// Module-specific biased NPC advice → biased_npc_advice
    #[serde(default)]
    pub npc_advice: Vec<NpcBiasedAdvice>,
    /// Module-specific known facts → brief.known_facts
    #[serde(default)]
    pub known_facts: Vec<String>,
    /// Module-specific open question → brief.open_questions
    #[serde(default)]
    pub open_question: Option<String>,
    /// Points-to tags for the open question (decision options).
    #[serde(default)]
    pub open_question_points_to: Vec<String>,
    /// Fallback place summary when state has no location_id.
    #[serde(default)]
    pub place_summary_fallback: Option<String>,
}

/// Lightweight module-level engine config. Lives in the module bundle; every
/// field #[serde(default)] so pre-P0-2 bundles deserialize unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct ModuleConfig {
    #[serde(default)]
    pub npc_actor_bindings: Vec<NpcActorBinding>,
    #[serde(default)]
    pub technical_option_table: Option<Vec<TechOption>>,
    #[serde(default)]
    pub scene_entity_aliases: Vec<EntityAlias>,
    #[serde(default)]
    pub module_search_profile: Option<SearchProfile>,
    /// P0-2 T4: director facilitation overlay (scene facts / NPC advice /
    /// place summary) that replaced trpg-director's is_homecoming hardcode.
    #[serde(default)]
    pub director: Option<DirectorModuleConfig>,
}

// ---------------------------------------------------------------------------
// P0-2 GENERIC_* neutral defaults — exact equivalents of the engine's legacy
// generic branches (trpg-combat::default_generic_profile, trpg-referee's
// generic ruleset_* fallthrough). Used when kernel.<field> is None. These are
// NEUTRAL (no ruleset/module name literals); CombatMode variant names are not
// ruleset names (whitelisted).
// ---------------------------------------------------------------------------

/// Neutral combat profile (mirrors trpg-combat::default_generic_profile's
/// fiction-first strategy). Used when kernel.combat_profile is None.
pub static GENERIC_COMBAT_PROFILE: std::sync::LazyLock<CombatProfile> =
    std::sync::LazyLock::new(|| CombatProfile {
        profile_id: "generic.situation.v1_3".into(),
        default_mode: "theater_of_mind".into(),
        applies_to_modes: vec![
            "theater_of_mind".into(),
            "tactical_combat".into(),
            "social_conflict".into(),
        ],
        action_economy: serde_json::json!({"policy":"fiction_first"}),
        initiative: serde_json::json!({"policy":"fiction_first"}),
        reaction_windows: vec![],
        frame_exit_policy: serde_json::json!({"state_exits":["objective_completed","side_escaped","negotiated_truce","surrender_accepted"],"stalemate_after_non_decisive_turns":3}),
        stalemate_policy: serde_json::json!({"max_repeated_action_count":2,"open_direction_gate":true}),
        npc_drive_policy: serde_json::json!({"default_patience":40,"default_morale":55,"max_repeat_same_tactic":2}),
        search_recipes: vec![],
    });

/// Neutral mode policy: no rules → always falls back to TheaterOfMind
/// (mirrors infer_combat_mode_from_intent's trailing default).
pub static GENERIC_COMBAT_MODE_POLICY: std::sync::LazyLock<CombatModePolicy> =
    std::sync::LazyLock::new(|| CombatModePolicy {
        rules: vec![],
        fallback_mode: CombatMode::TheaterOfMind,
    });

/// Neutral referee bands (mirrors trpg-referee's generic branch verbatim:
/// damage 1..=30 dice, target 1..=100).
pub static GENERIC_REFEREE_BANDS: std::sync::LazyLock<RefereeValueBands> =
    std::sync::LazyLock::new(|| RefereeValueBands {
        damage_family: "generic_trpg_damage".into(),
        damage_band: "system-specific; exact object/ability entry required".into(),
        damage_plausible_range: (1, 30),
        difficulty_band: serde_json::json!({"common_target_band":"ruleset-specific"}),
        difficulty_plausible_range: (1, 100),
    });

/// Neutral dice qualification: bare "1d10" → "1d10+0" (mirrors the hardcode).
pub static GENERIC_DICE_QUALIFICATION: std::sync::LazyLock<DiceQualification> =
    std::sync::LazyLock::new(|| DiceQualification {
        bare_dice_template: "{dice}+0".into(),
    });

/// Empty check-label policy → engine uses its generic per-family label.
pub static GENERIC_CHECK_LABEL_POLICY: std::sync::LazyLock<CheckLabelPolicy> =
    std::sync::LazyLock::new(CheckLabelPolicy::default);

/// Empty search profile → engine uses generic base sections/aliases for every skill.
pub static GENERIC_SEARCH_PROFILE: std::sync::LazyLock<RuleKernelSearchProfile> =
    std::sync::LazyLock::new(RuleKernelSearchProfile::default);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CombatPhase {
    NotStarted,
    EstablishingScene,
    DeterminingSurprise,
    RollingInitiative,
    AwaitingActorAction,
    AwaitingRequiredReaction,
    AwaitingRoll,
    ResolvingAction,
    ApplyingEffects,
    EvaluatingFrame,
    ExitAttempt,
    Deescalation,
    Stalemate,
    DirectionGate,
    AdvancingTurn,
    RoundEnd,
    CombatEndPendingCompaction,
    Completed,
}

impl Default for CombatPhase {
    fn default() -> Self { Self::NotStarted }
}

impl CombatPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            CombatPhase::NotStarted => "not_started",
            CombatPhase::EstablishingScene => "establishing_scene",
            CombatPhase::DeterminingSurprise => "determining_surprise",
            CombatPhase::RollingInitiative => "rolling_initiative",
            CombatPhase::AwaitingActorAction => "awaiting_actor_action",
            CombatPhase::AwaitingRequiredReaction => "awaiting_required_reaction",
            CombatPhase::AwaitingRoll => "awaiting_roll",
            CombatPhase::ResolvingAction => "resolving_action",
            CombatPhase::ApplyingEffects => "applying_effects",
            CombatPhase::EvaluatingFrame => "evaluating_frame",
            CombatPhase::ExitAttempt => "exit_attempt",
            CombatPhase::Deescalation => "deescalation",
            CombatPhase::Stalemate => "stalemate",
            CombatPhase::DirectionGate => "direction_gate",
            CombatPhase::AdvancingTurn => "advancing_turn",
            CombatPhase::RoundEnd => "round_end",
            CombatPhase::CombatEndPendingCompaction => "combat_end_pending_compaction",
            CombatPhase::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct InitiativeSlot {
    pub actor_id: String,
    pub display_name: Option<String>,
    pub initiative_total: Option<i32>,
    pub acted_this_round: bool,
    pub metadata: serde_json::Value,
}


// -----------------------------------------------------------------------------
// Semantic Situation Orchestrator / Novelty Director data contracts (v1.1-v1.3)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameRelation {
    InsideFrameAction,
    GateResponse,
    ExitAttempt,
    DeescalationAttempt,
    Surrender,
    Flee,
    HideToDisengage,
    PauseAndObserve,
    OutsideFrameAction,
    Clarification,
    InvalidOrAmbiguous,
}

impl Default for FrameRelation {
    fn default() -> Self { Self::InvalidOrAmbiguous }
}

impl FrameRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameRelation::InsideFrameAction => "inside_frame_action",
            FrameRelation::GateResponse => "gate_response",
            FrameRelation::ExitAttempt => "exit_attempt",
            FrameRelation::DeescalationAttempt => "deescalation_attempt",
            FrameRelation::Surrender => "surrender",
            FrameRelation::Flee => "flee",
            FrameRelation::HideToDisengage => "hide_to_disengage",
            FrameRelation::PauseAndObserve => "pause_and_observe",
            FrameRelation::OutsideFrameAction => "outside_frame_action",
            FrameRelation::Clarification => "clarification",
            FrameRelation::InvalidOrAmbiguous => "invalid_or_ambiguous",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SituationActionKind {
    Attack,
    UnderAttack,
    EnemyInitiatedConflict,
    SceneEntersConflict,
    Defend,
    Dodge,
    Counterattack,
    TakeCover,
    Aim,
    Move,
    Flee,
    Hide,
    Negotiate,
    Surrender,
    Intimidate,
    Hack,
    DisableDevice,
    Rescue,
    ActivateAbility,
    TriggerAbility,
    UseItem,
    CastOrUsePower,
    InvestigateDuringConflict,
    WaitOrHoldAction,
    AskQuestion,
    Refuse,
    AcceptDeal,
    LeaveScene,
    EndConflict,
    Unknown,
}

impl Default for SituationActionKind {
    fn default() -> Self { Self::Unknown }
}

impl SituationActionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SituationActionKind::Attack => "attack",
            SituationActionKind::UnderAttack => "under_attack",
            SituationActionKind::EnemyInitiatedConflict => "enemy_initiated_conflict",
            SituationActionKind::SceneEntersConflict => "scene_enters_conflict",
            SituationActionKind::Defend => "defend",
            SituationActionKind::Dodge => "dodge",
            SituationActionKind::Counterattack => "counterattack",
            SituationActionKind::TakeCover => "take_cover",
            SituationActionKind::Aim => "aim",
            SituationActionKind::Move => "move",
            SituationActionKind::Flee => "flee",
            SituationActionKind::Hide => "hide",
            SituationActionKind::Negotiate => "negotiate",
            SituationActionKind::Surrender => "surrender",
            SituationActionKind::Intimidate => "intimidate",
            SituationActionKind::Hack => "hack",
            SituationActionKind::DisableDevice => "disable_device",
            SituationActionKind::Rescue => "rescue",
            SituationActionKind::ActivateAbility => "activate_ability",
            SituationActionKind::TriggerAbility => "trigger_ability",
            SituationActionKind::UseItem => "use_item",
            SituationActionKind::CastOrUsePower => "cast_or_use_power",
            SituationActionKind::InvestigateDuringConflict => "investigate_during_conflict",
            SituationActionKind::WaitOrHoldAction => "wait_or_hold_action",
            SituationActionKind::AskQuestion => "ask_question",
            SituationActionKind::Refuse => "refuse",
            SituationActionKind::AcceptDeal => "accept_deal",
            SituationActionKind::LeaveScene => "leave_scene",
            SituationActionKind::EndConflict => "end_conflict",
            SituationActionKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EscalationLevel {
    Low,
    Medium,
    High,
    Lethal,
}

impl Default for EscalationLevel {
    fn default() -> Self { Self::Low }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameOutcome {
    Continue,
    ContinueWithPromptGate,
    PlayerEscaped,
    EnemyEscaped,
    PlayerDefeated,
    EnemyDefeated,
    ObjectiveCompleted,
    ObjectiveFailed,
    NegotiatedTruce,
    SurrenderAccepted,
    SurrenderRejected,
    TransitionToChase,
    TransitionToSocialConflict,
    TransitionToHazardSequence,
    StalemateNeedsDirection,
    Paused,
    Abandoned,
    Closed,
}

impl Default for FrameOutcome {
    fn default() -> Self { Self::Continue }
}

impl FrameOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            FrameOutcome::Continue => "continue",
            FrameOutcome::ContinueWithPromptGate => "continue_with_prompt_gate",
            FrameOutcome::PlayerEscaped => "player_escaped",
            FrameOutcome::EnemyEscaped => "enemy_escaped",
            FrameOutcome::PlayerDefeated => "player_defeated",
            FrameOutcome::EnemyDefeated => "enemy_defeated",
            FrameOutcome::ObjectiveCompleted => "objective_completed",
            FrameOutcome::ObjectiveFailed => "objective_failed",
            FrameOutcome::NegotiatedTruce => "negotiated_truce",
            FrameOutcome::SurrenderAccepted => "surrender_accepted",
            FrameOutcome::SurrenderRejected => "surrender_rejected",
            FrameOutcome::TransitionToChase => "transition_to_chase",
            FrameOutcome::TransitionToSocialConflict => "transition_to_social_conflict",
            FrameOutcome::TransitionToHazardSequence => "transition_to_hazard_sequence",
            FrameOutcome::StalemateNeedsDirection => "stalemate_needs_direction",
            FrameOutcome::Paused => "paused",
            FrameOutcome::Abandoned => "abandoned",
            FrameOutcome::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ConflictIntent {
    pub intent_id: String,
    pub language: Option<String>,
    pub relation_to_active_frame: FrameRelation,
    pub action_kind: SituationActionKind,
    pub escalation_level: EscalationLevel,
    pub target_refs: Vec<String>,
    pub desired_outcome: Option<String>,
    pub confidence: RulingConfidence,
    pub evidence_terms: Vec<String>,
    pub classifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FrameProgressTracker {
    pub turns_elapsed: u32,
    pub turns_since_decisive_change: u32,
    pub rounds_since_decisive_change: u32,
    pub repeated_action_count: u32,
    pub last_action_kind: Option<SituationActionKind>,
    pub last_decisive_event_id: Option<String>,
    pub objective_progress_delta: i32,
    pub harm_or_resource_delta: i32,
    pub information_delta: i32,
    pub position_delta: i32,
    pub relationship_delta: i32,
    pub threat_delta: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct NpcDriveState {
    pub npc_id: String,
    pub current_goal: String,
    pub current_tactic: String,
    pub patience: i32,
    pub morale: i32,
    pub fear: i32,
    pub aggression: i32,
    pub suspicion: i32,
    pub loyalty: i32,
    pub risk_tolerance: i32,
    pub self_interest: i32,
    pub fanaticism: i32,
    pub repeated_strategy_count: u32,
    pub last_progress_turn: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct NpcTacticMemory {
    pub npc_id: String,
    pub tactic_id: String,
    pub used_count_in_frame: u32,
    pub consecutive_count: u32,
    pub last_result: Option<String>,
    pub last_turn_id: Option<String>,
}


#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FreshChangeType {
    NewPressure,
    NewRisk,
    NewAffordance,
    NpcTacticShift,
    ClockTick,
    ResourceCost,
    PositionChange,
    ObjectiveProgress,
    ThreatRevealed,
    ConsequenceEscalation,
    ContinuedPressureWithCost,
}

impl Default for FreshChangeType {
    fn default() -> Self { Self::NewPressure }
}

impl FreshChangeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FreshChangeType::NewPressure => "new_pressure",
            FreshChangeType::NewRisk => "new_risk",
            FreshChangeType::NewAffordance => "new_affordance",
            FreshChangeType::NpcTacticShift => "npc_tactic_shift",
            FreshChangeType::ClockTick => "clock_tick",
            FreshChangeType::ResourceCost => "resource_cost",
            FreshChangeType::PositionChange => "position_change",
            FreshChangeType::ObjectiveProgress => "objective_progress",
            FreshChangeType::ThreatRevealed => "threat_revealed",
            FreshChangeType::ConsequenceEscalation => "consequence_escalation",
            FreshChangeType::ContinuedPressureWithCost => "continued_pressure_with_cost",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct FreshChange {
    pub change_id: String,
    pub change_type: FreshChangeType,
    pub summary: String,
    pub caused_by: String,
    pub source_refs: Vec<SourceRef>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BeatSignature {
    pub beat_id: String,
    pub actor_id: String,
    pub tactic_id: String,
    pub target_id: Option<String>,
    pub approach_vector: String,
    pub consequence_type: String,
    pub scene_element_used: Option<String>,
    pub emotional_tone: Option<String>,
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TacticCooldown {
    pub actor_id: String,
    pub tactic_id: String,
    pub cooldown_remaining_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct NpcTactic {
    pub tactic_id: String,
    pub label: String,
    pub use_when: Vec<String>,
    pub avoid_when: Vec<String>,
    pub creates: Vec<String>,
    pub novelty_tags: Vec<String>,
    pub cooldown_after_use: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TacticPalette {
    pub actor_id: String,
    pub archetype: String,
    pub max_repeat_same_tactic: u32,
    pub tactics: Vec<NpcTactic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct NoveltyState {
    pub recent_beats: Vec<BeatSignature>,
    pub tactic_cooldowns: Vec<TacticCooldown>,
    pub repeated_output_closure_count: u32,
    pub forced_tactic_shift_count: u32,
    pub last_fresh_change: Option<FreshChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct NoveltyDecision {
    pub decision_id: String,
    pub force_tactic_shift: bool,
    pub selected_tactic_id: Option<String>,
    pub previous_tactic_id: Option<String>,
    pub novelty_score: f32,
    pub reason: String,
    pub fresh_change: Option<FreshChange>,
    pub beat: Option<BeatSignature>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CombatantStatus {
    Active,
    TakingCover,
    Hidden,
    Disengaging,
    Fleeing,
    Surrendering,
    Negotiating,
    Incapacitated,
    Unconscious,
    MortallyWounded,
    Dead,
    Escaped,
    Captured,
}

impl Default for CombatantStatus {
    fn default() -> Self { Self::Active }
}

impl CombatantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CombatantStatus::Active => "active",
            CombatantStatus::TakingCover => "taking_cover",
            CombatantStatus::Hidden => "hidden",
            CombatantStatus::Disengaging => "disengaging",
            CombatantStatus::Fleeing => "fleeing",
            CombatantStatus::Surrendering => "surrendering",
            CombatantStatus::Negotiating => "negotiating",
            CombatantStatus::Incapacitated => "incapacitated",
            CombatantStatus::Unconscious => "unconscious",
            CombatantStatus::MortallyWounded => "mortally_wounded",
            CombatantStatus::Dead => "dead",
            CombatantStatus::Escaped => "escaped",
            CombatantStatus::Captured => "captured",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    FleeArea,
    HideAndDisengage,
    TakeCoverAndPause,
    Surrender,
    Ceasefire,
    Negotiate,
    IntimidateToEnd,
    ObjectiveCompleteAndWithdraw,
    LeaveScene,
}

impl Default for ExitKind {
    fn default() -> Self { Self::LeaveScene }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpponentResponsePolicy {
    NoResponseNeeded,
    MayPursue,
    MayRefuse,
    RequiresMoraleOrPatienceCheck,
    TransitionToChase,
    TransitionToSocialConflict,
}

impl Default for OpponentResponsePolicy {
    fn default() -> Self { Self::NoResponseNeeded }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ExitContract {
    pub exit_id: String,
    pub frame_id: String,
    pub exit_kind: ExitKind,
    pub actor_id: String,
    pub target_side: Option<String>,
    pub method_summary: String,
    pub requires_check: bool,
    pub check_contract: Option<CheckContract>,
    pub opponent_response: OpponentResponsePolicy,
    pub success_outcome: FrameOutcome,
    pub failure_outcome: FrameOutcome,
    pub source_refs: Vec<SourceRef>,
    pub advice_refs: Vec<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct StalemateContract {
    pub stalemate_id: String,
    pub frame_id: String,
    pub reason: String,
    pub turns_since_decisive_change: u32,
    pub repeated_action_count: u32,
    pub direction_options: Vec<ActionOption>,
    pub recommended_gate_kind: GateKind,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatParticipantState {
    pub actor_id: String,
    pub actor_kind: ActorKind,
    pub display_name: Option<String>,
    pub side_id: String,
    pub hp_current: Option<i32>,
    pub hp_max: Option<i32>,
    pub armor_current: Option<i32>,
    pub statuses: Vec<String>,
    pub resources: serde_json::Value,
    pub position: Option<String>,
    pub visible_to_players: bool,
    #[serde(default)]
    pub combat_status: CombatantStatus,
    #[serde(default)]
    pub morale: Option<i32>,
    #[serde(default)]
    pub patience: Option<i32>,
    #[serde(default)]
    pub last_tactic_id: Option<String>,
    #[serde(default)]
    pub repeated_tactic_count: u32,
    #[serde(default)]
    pub turns_since_progress: u32,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatSide {
    pub side_id: String,
    pub label: String,
    pub actor_ids: Vec<String>,
    pub disposition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatZone {
    pub zone_id: String,
    pub label: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RangeModel {
    pub model_id: String,
    pub kind: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActionSlotState {
    pub slot_id: String,
    pub label: String,
    pub max_uses: i32,
    pub used: i32,
    pub refresh: String,
    pub requires_feature: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActionEconomyState {
    pub profile_id: String,
    pub actor_slots: BTreeMap<String, Vec<ActionSlotState>>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ReactionWindow {
    pub window_id: String,
    pub trigger_event_id: Option<String>,
    pub target_actor_id: String,
    pub source_actor_id: Option<String>,
    pub required: bool,
    pub options: Vec<ActionOption>,
    pub default_if_unanswered: Option<String>,
    pub expires_at_phase: Option<CombatPhase>,
    pub gate_id: Option<String>,
    pub source_refs: Vec<SourceRef>,
    pub advice_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TemporaryEffect {
    pub effect_id: String,
    pub summary: String,
    pub target_actor_ids: Vec<String>,
    pub expires_when: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatHazard {
    pub hazard_id: String,
    pub summary: String,
    pub clock_id: Option<String>,
    pub visibility: Visibility,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatObjective {
    pub objective_id: String,
    pub summary: String,
    pub owner_side_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatWorkingState {
    pub combat_id: String,
    pub combat_mode: CombatMode,
    pub round: u32,
    pub phase: CombatPhase,
    pub active_actor_id: Option<String>,
    pub initiative_order: Vec<InitiativeSlot>,
    pub participants: Vec<CombatParticipantState>,
    pub sides: Vec<CombatSide>,
    pub zones: Vec<CombatZone>,
    pub range_model: RangeModel,
    pub action_economy: ActionEconomyState,
    pub reaction_windows: Vec<ReactionWindow>,
    pub active_effects: Vec<TemporaryEffect>,
    pub hazards: Vec<CombatHazard>,
    pub objectives: Vec<CombatObjective>,
    pub rule_packet_ids: Vec<String>,
    pub encounter_packet_ids: Vec<String>,
    pub last_events: Vec<String>,
    #[serde(default)]
    pub progress_tracker: FrameProgressTracker,
    #[serde(default)]
    pub npc_drive_states: Vec<NpcDriveState>,
    #[serde(default)]
    pub npc_tactic_memory: Vec<NpcTacticMemory>,
    #[serde(default)]
    pub novelty_state: NoveltyState,
    #[serde(default)]
    pub tactic_palettes: Vec<TacticPalette>,
    #[serde(default)]
    pub current_outcome: Option<FrameOutcome>,
    #[serde(default)]
    pub last_intent: Option<ConflictIntent>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Damage,
    ArmorOrResistance,
    Healing,
    Condition,
    ResourceSpend,
    PositionChange,
    MoraleChange,
    SanityLoss,
    ChaosGain,
    ChaosSpend,
    LooseEnd,
    CriticalInjury,
    MajorWound,
    DeathOrDying,
    NarrativeConsequence,
}

impl Default for EffectKind {
    fn default() -> Self { Self::NarrativeConsequence }
}

impl EffectKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EffectKind::Damage => "damage",
            EffectKind::ArmorOrResistance => "armor_or_resistance",
            EffectKind::Healing => "healing",
            EffectKind::Condition => "condition",
            EffectKind::ResourceSpend => "resource_spend",
            EffectKind::PositionChange => "position_change",
            EffectKind::MoraleChange => "morale_change",
            EffectKind::SanityLoss => "sanity_loss",
            EffectKind::ChaosGain => "chaos_gain",
            EffectKind::ChaosSpend => "chaos_spend",
            EffectKind::LooseEnd => "loose_end",
            EffectKind::CriticalInjury => "critical_injury",
            EffectKind::MajorWound => "major_wound",
            EffectKind::DeathOrDying => "death_or_dying",
            EffectKind::NarrativeConsequence => "narrative_consequence",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ResolvedEffectPart {
    pub label: String,
    pub value_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct EffectRollRequest {
    pub label: String,
    pub dice_expression: String,
    pub roll_visibility: RollVisibility,
    pub target_actor_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct EffectContract {
    pub effect_id: String,
    pub source_event_id: Option<String>,
    pub effect_kind: EffectKind,
    pub target_actor_ids: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub learned_packet_ids: Vec<String>,
    pub deterministic_parts: Vec<ResolvedEffectPart>,
    pub pending_rolls: Vec<EffectRollRequest>,
    pub proposed_patches: Vec<StatePatch>,
    pub visibility: Visibility,
    pub confidence: RulingConfidence,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatProfileSummary {
    pub profile_id: String,
    pub ruleset_id: String,
    pub applies_to_modes: Vec<CombatMode>,
    pub initiative_policy: serde_json::Value,
    pub action_economy: serde_json::Value,
    pub reaction_windows: Vec<serde_json::Value>,
    pub attack_resolution: Vec<serde_json::Value>,
    pub damage_resolution: Vec<serde_json::Value>,
    pub frame_retention_policy: RetentionPolicy,
    pub frame_compaction_policy: CompactionPolicy,
    pub search_recipes: Vec<serde_json::Value>,
    pub source_refs: Vec<SourceRef>,
}


// -----------------------------------------------------------------------------
// Object & Possession Kernel data contracts (v1.6)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Weapon,
    Armor,
    Shield,
    Tool,
    Consumable,
    Ammo,
    Container,
    Door,
    Lock,
    Vehicle,
    VehiclePart,
    Cyberware,
    Program,
    Device,
    Document,
    Key,
    Clue,
    Currency,
    QuestItem,
    EnvironmentalFeature,
    Structure,
    HazardObject,
    AnomalyObject,
    Unknown,
}
impl Default for ObjectKind { fn default() -> Self { Self::Unknown } }
impl ObjectKind { pub fn as_str(&self) -> &'static str { match self { ObjectKind::Weapon => "weapon", ObjectKind::Armor => "armor", ObjectKind::Shield => "shield", ObjectKind::Tool => "tool", ObjectKind::Consumable => "consumable", ObjectKind::Ammo => "ammo", ObjectKind::Container => "container", ObjectKind::Door => "door", ObjectKind::Lock => "lock", ObjectKind::Vehicle => "vehicle", ObjectKind::VehiclePart => "vehicle_part", ObjectKind::Cyberware => "cyberware", ObjectKind::Program => "program", ObjectKind::Device => "device", ObjectKind::Document => "document", ObjectKind::Key => "key", ObjectKind::Clue => "clue", ObjectKind::Currency => "currency", ObjectKind::QuestItem => "quest_item", ObjectKind::EnvironmentalFeature => "environmental_feature", ObjectKind::Structure => "structure", ObjectKind::HazardObject => "hazard_object", ObjectKind::AnomalyObject => "anomaly_object", ObjectKind::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EquipSlotKind { MainHand, OffHand, TwoHands, BodyArmor, Head, Body, Accessory, Backpack, VehicleMount, CyberdeckSlot, ProgramSlot, Custom }
impl Default for EquipSlotKind { fn default() -> Self { Self::Custom } }
impl EquipSlotKind { pub fn as_str(&self) -> &'static str { match self { EquipSlotKind::MainHand => "main_hand", EquipSlotKind::OffHand => "off_hand", EquipSlotKind::TwoHands => "two_hands", EquipSlotKind::BodyArmor => "body_armor", EquipSlotKind::Head => "head", EquipSlotKind::Body => "body", EquipSlotKind::Accessory => "accessory", EquipSlotKind::Backpack => "backpack", EquipSlotKind::VehicleMount => "vehicle_mount", EquipSlotKind::CyberdeckSlot => "cyberdeck_slot", EquipSlotKind::ProgramSlot => "program_slot", EquipSlotKind::Custom => "custom" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandSlot { Left, Right, Both, Unknown }
impl Default for HandSlot { fn default() -> Self { Self::Unknown } }
impl HandSlot { pub fn as_str(&self) -> &'static str { match self { HandSlot::Left => "left", HandSlot::Right => "right", HandSlot::Both => "both", HandSlot::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectInteractionKind {
    GrabHeldObject,
    Disarm,
    Steal,
    PickUp,
    Drop,
    Equip,
    Unequip,
    Draw,
    Sheathe,
    Reload,
    Throw,
    Use,
    Activate,
    Deactivate,
    Break,
    Repair,
    Hack,
    Search,
    Hide,
    Reveal,
    Lock,
    Unlock,
    Install,
    Remove,
    Transfer,
    Loot,
    Inspect,
    TraceConnection,
    CutConnection,
    Unknown,
}
impl Default for ObjectInteractionKind { fn default() -> Self { Self::Unknown } }
impl ObjectInteractionKind { pub fn as_str(&self) -> &'static str { match self { ObjectInteractionKind::GrabHeldObject => "grab_held_object", ObjectInteractionKind::Disarm => "disarm", ObjectInteractionKind::Steal => "steal", ObjectInteractionKind::PickUp => "pick_up", ObjectInteractionKind::Drop => "drop", ObjectInteractionKind::Equip => "equip", ObjectInteractionKind::Unequip => "unequip", ObjectInteractionKind::Draw => "draw", ObjectInteractionKind::Sheathe => "sheathe", ObjectInteractionKind::Reload => "reload", ObjectInteractionKind::Throw => "throw", ObjectInteractionKind::Use => "use", ObjectInteractionKind::Activate => "activate", ObjectInteractionKind::Deactivate => "deactivate", ObjectInteractionKind::Break => "break", ObjectInteractionKind::Repair => "repair", ObjectInteractionKind::Hack => "hack", ObjectInteractionKind::Search => "search", ObjectInteractionKind::Hide => "hide", ObjectInteractionKind::Reveal => "reveal", ObjectInteractionKind::Lock => "lock", ObjectInteractionKind::Unlock => "unlock", ObjectInteractionKind::Install => "install", ObjectInteractionKind::Remove => "remove", ObjectInteractionKind::Transfer => "transfer", ObjectInteractionKind::Loot => "loot", ObjectInteractionKind::Inspect => "inspect", ObjectInteractionKind::TraceConnection => "trace_connection", ObjectInteractionKind::CutConnection => "cut_connection", ObjectInteractionKind::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectRuleTrigger { OnEquip, OnUnequip, OnAttack, OnHit, OnDamageRoll, OnDefense, OnTakeDamage, OnReload, OnMalfunction, OnConcealReveal, OnBreak, OnRepair, OnDisarmAttempt, OnDropped, OnPickedUp, OnTimeTick, OnFrameCompaction }
impl Default for ObjectRuleTrigger { fn default() -> Self { Self::OnPickedUp } }
impl ObjectRuleTrigger { pub fn as_str(&self) -> &'static str { match self { ObjectRuleTrigger::OnEquip => "on_equip", ObjectRuleTrigger::OnUnequip => "on_unequip", ObjectRuleTrigger::OnAttack => "on_attack", ObjectRuleTrigger::OnHit => "on_hit", ObjectRuleTrigger::OnDamageRoll => "on_damage_roll", ObjectRuleTrigger::OnDefense => "on_defense", ObjectRuleTrigger::OnTakeDamage => "on_take_damage", ObjectRuleTrigger::OnReload => "on_reload", ObjectRuleTrigger::OnMalfunction => "on_malfunction", ObjectRuleTrigger::OnConcealReveal => "on_conceal_reveal", ObjectRuleTrigger::OnBreak => "on_break", ObjectRuleTrigger::OnRepair => "on_repair", ObjectRuleTrigger::OnDisarmAttempt => "on_disarm_attempt", ObjectRuleTrigger::OnDropped => "on_dropped", ObjectRuleTrigger::OnPickedUp => "on_picked_up", ObjectRuleTrigger::OnTimeTick => "on_time_tick", ObjectRuleTrigger::OnFrameCompaction => "on_frame_compaction" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectRuleBinding {
    pub binding_id: String,
    pub trigger: ObjectRuleTrigger,
    pub rule_packet_id: Option<String>,
    pub lookup_recipe_id: Option<String>,
    pub source_refs: Vec<SourceRef>,
    pub effect_json: serde_json::Value,
    pub confidence: RulingConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectDefinition {
    pub object_def_id: String,
    pub ruleset_id: String,
    pub name: String,
    pub object_kind: ObjectKind,
    pub tags: Vec<String>,
    pub mechanical_profile: serde_json::Value,
    pub rule_bindings: Vec<ObjectRuleBinding>,
    pub default_affordances: Vec<ObjectInteractionKind>,
    pub equip_slots: Vec<EquipSlotKind>,
    pub visibility_default: Visibility,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DurabilityState { pub current: i32, pub max: i32, pub broken: bool }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState { Unknown, VisibleUnidentified, Suspected, Known, Identified, Hidden, GmOnly, Playwalled }
impl Default for DiscoveryState { fn default() -> Self { Self::Unknown } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectVisibilityState {
    pub public_label: Option<String>,
    pub identified_label: Option<String>,
    pub gm_label: String,
    pub discovery_state: DiscoveryState,
    pub known_by_actor_ids: Vec<String>,
    pub known_by_player: bool,
    pub reveal_conditions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectLocation {
    Held { actor_id: String, hand: Option<HandSlot> },
    Equipped { actor_id: String, slot: EquipSlotKind },
    Worn { actor_id: String, slot: EquipSlotKind },
    Carried { actor_id: String, container_id: Option<String> },
    InContainer { container_id: String },
    Installed { parent_object_id: String, port_or_mount: String },
    Attached { target_id: String, attachment_kind: String },
    Connected { endpoint_a: String, endpoint_b: String, connection_kind: String },
    OnGround { scene_id: String, zone_id: Option<String> },
    Hidden { scope_id: String, concealment: String },
    ClaimedByActor { actor_id: String, claim_kind: String },
    Destroyed,
    Unknown,
}
impl Default for ObjectLocation { fn default() -> Self { Self::Unknown } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectRelation { Contains, LoadedWith, InstalledIn, AttachedTo, ConnectedTo, Controls, Locks, Secures, Conceals, Powers, MountedOn, WornBy, HeldBy, ClaimedBy, BelongsTo }
impl Default for ObjectRelation { fn default() -> Self { Self::Contains } }
impl ObjectRelation { pub fn as_str(&self) -> &'static str { match self { ObjectRelation::Contains => "contains", ObjectRelation::LoadedWith => "loaded_with", ObjectRelation::InstalledIn => "installed_in", ObjectRelation::AttachedTo => "attached_to", ObjectRelation::ConnectedTo => "connected_to", ObjectRelation::Controls => "controls", ObjectRelation::Locks => "locks", ObjectRelation::Secures => "secures", ObjectRelation::Conceals => "conceals", ObjectRelation::Powers => "powers", ObjectRelation::MountedOn => "mounted_on", ObjectRelation::WornBy => "worn_by", ObjectRelation::HeldBy => "held_by", ObjectRelation::ClaimedBy => "claimed_by", ObjectRelation::BelongsTo => "belongs_to" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectInstance {
    pub object_id: String,
    pub object_def_id: Option<String>,
    pub session_id: String,
    pub scope: Scope,
    pub display_name: String,
    pub object_kind: ObjectKind,
    pub location: ObjectLocation,
    pub visibility_state: ObjectVisibilityState,
    pub quantity: Option<i32>,
    pub durability: Option<DurabilityState>,
    pub mechanical_state: serde_json::Value,
    pub tags: Vec<String>,
    pub created_at_tick: i64,
    pub updated_at_tick: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectEdge {
    pub edge_id: String,
    pub session_id: String,
    pub from_object_id: String,
    pub to_object_id: String,
    pub relation: ObjectRelation,
    pub metadata: serde_json::Value,
    pub visibility: Visibility,
    pub valid_from_tick: i64,
    pub valid_until_tick: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectPrecondition {
    ObjectExists { object_id: String },
    ObjectVisibleOrKnown { object_id: String, actor_id: String },
    ObjectWithinReach { object_id: String, actor_id: String },
    ObjectHeldBy { object_id: String, actor_id: String },
    ActorHasFreeHand { actor_id: String },
    ActorHasSlotFree { actor_id: String, slot: EquipSlotKind },
    ObjectNotLocked { object_id: String },
    ObjectNotSecured { object_id: String },
    ObjectPortable { object_id: String },
    ObjectCanBeRemoved { object_id: String },
    RequiresToolOrSkill { object_id: String, requirement: String },
    RequiresDiscovery { object_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectPatch {
    CreateObjectInstance { object: ObjectInstance, reason: String },
    TransferObject { object_id: String, from: ObjectLocation, to: ObjectLocation, reason: String },
    SetObjectLocation { object_id: String, location: ObjectLocation, reason: String },
    SetObjectVisibility { object_id: String, visibility_state: ObjectVisibilityState, reason: String },
    ModifyQuantity { object_id: String, delta: i32, reason: String },
    DamageObject { object_id: String, amount: i32, damage_kind: String, reason: String },
    DestroyObject { object_id: String, reason: String },
    AddObjectEdge { edge: ObjectEdge, reason: String },
    RemoveObjectEdge { edge_id: String, reason: String },
    SetMechanicalState { object_id: String, patch_json: serde_json::Value, reason: String },
    /// Turn an existing object into a different one in place (e.g. a depleted
    /// spell wand -> an inert stick): change its kind/name and REPLACE its
    /// mechanical state. The object_id is preserved (edges/possession survive).
    TransformObject { object_id: String, new_kind: Option<ObjectKind>, new_name: Option<String>, set_mechanical_state: Option<serde_json::Value>, reason: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectInteractionStatus { Created, AwaitingCheck, Resolved, Failed, Applied, Superseded, Cancelled }
impl Default for ObjectInteractionStatus { fn default() -> Self { Self::Created } }
impl ObjectInteractionStatus { pub fn as_str(&self) -> &'static str { match self { ObjectInteractionStatus::Created => "created", ObjectInteractionStatus::AwaitingCheck => "awaiting_check", ObjectInteractionStatus::Resolved => "resolved", ObjectInteractionStatus::Failed => "failed", ObjectInteractionStatus::Applied => "applied", ObjectInteractionStatus::Superseded => "superseded", ObjectInteractionStatus::Cancelled => "cancelled" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectInteractionContract {
    pub interaction_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub interaction_context_id: Option<String>,
    pub actor_id: String,
    pub target_actor_id: Option<String>,
    pub target_object_id: Option<String>,
    pub interaction_kind: ObjectInteractionKind,
    pub preconditions: Vec<ObjectPrecondition>,
    pub check_contract: Option<CheckContract>,
    pub effect_contracts: Vec<EffectContract>,
    pub success_patches: Vec<ObjectPatch>,
    pub failure_patches: Vec<ObjectPatch>,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub learned_packet_ids: Vec<String>,
    pub advice_refs: Vec<String>,
    pub status: ObjectInteractionStatus,
    pub created_at_tick: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectAffordance {
    pub affordance_id: String,
    pub object_id: String,
    pub actor_id: String,
    pub action_kind: ObjectInteractionKind,
    pub label: String,
    pub description: String,
    pub requires_check: bool,
    pub check_recipe: Option<String>,
    pub risk_level: String,
    pub visible_to_player: bool,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ObjectEvent {
    pub object_event_id: String,
    pub session_id: String,
    pub object_id: Option<String>,
    pub frame_id: Option<String>,
    pub world_event_id: Option<String>,
    pub event_kind: String,
    pub event_json: serde_json::Value,
    pub visibility: Visibility,
    pub world_tick: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ObjectInteractionResult {
    pub interaction_id: String,
    pub check_id: Option<String>,
    pub applied_patches: Vec<ObjectPatch>,
    pub object_events: Vec<ObjectEvent>,
    pub status: ObjectInteractionStatus,
}


// -----------------------------------------------------------------------------
// Actionable Situation Director / Novelty Director data contracts (v1.2-v1.3)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionVector {
    Observe,
    Social,
    Technical,
    Tactical,
    Resource,
    Mobility,
    Stealth,
    Weird,
    Retreat,
    Safety,
    CharacterSpotlight,
}

impl Default for ActionVector { fn default() -> Self { Self::Observe } }
impl ActionVector {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionVector::Observe => "observe",
            ActionVector::Social => "social",
            ActionVector::Technical => "technical",
            ActionVector::Tactical => "tactical",
            ActionVector::Resource => "resource",
            ActionVector::Mobility => "mobility",
            ActionVector::Stealth => "stealth",
            ActionVector::Weird => "weird",
            ActionVector::Retreat => "retreat",
            ActionVector::Safety => "safety",
            ActionVector::CharacterSpotlight => "character_spotlight",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceLevel {
    ObservableFactsOnly,
    SummarizeKnownInfo,
    AskGoal,
    OfferActionCategories,
    OfferCostedExamples,
}

impl Default for GuidanceLevel { fn default() -> Self { Self::ObservableFactsOnly } }
impl GuidanceLevel { pub fn as_str(&self) -> &'static str { match self { GuidanceLevel::ObservableFactsOnly => "observable_facts_only", GuidanceLevel::SummarizeKnownInfo => "summarize_known_info", GuidanceLevel::AskGoal => "ask_goal", GuidanceLevel::OfferActionCategories => "offer_action_categories", GuidanceLevel::OfferCostedExamples => "offer_costed_examples" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerDecisionPromptKind { ChooseGoal, ChooseRiskTolerance, ChooseApproachVector, ChooseSpotlightFirstActor, ClarifyIntent }
impl Default for PlayerDecisionPromptKind { fn default() -> Self { Self::ChooseGoal } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScenePurpose { RevealInformation, ForceChoice, SpendResource, ShowConsequence, SpotlightCharacter, RaisePressure, ResolveConflict, TransitionLocation, EstablishTone }
impl Default for ScenePurpose { fn default() -> Self { Self::ForceChoice } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct VisibleFact {
    pub fact_id: String,
    pub text: String,
    pub source_refs: Vec<SourceRef>,
    pub confidence: RulingConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PressureItem {
    pub pressure_id: String,
    pub text: String,
    pub clock_id: Option<String>,
    pub severity: i32,
    pub consequence_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct Affordance {
    pub affordance_id: String,
    pub description: String,
    pub implies_vectors: Vec<ActionVector>,
    pub visible_to_players: bool,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RiskItem {
    pub risk_id: String,
    pub text: String,
    pub related_vectors: Vec<ActionVector>,
    pub severity: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct KnownFact {
    pub fact_id: String,
    pub text: String,
    pub source: String,
    pub public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct OpenQuestion {
    pub question_id: String,
    pub text: String,
    pub points_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct Lead {
    pub lead_id: String,
    pub text: String,
    pub target_ref: Option<String>,
    pub exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayerFacingClueBoard {
    pub board_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub known_facts: Vec<KnownFact>,
    pub open_leads: Vec<Lead>,
    pub unresolved_questions: Vec<OpenQuestion>,
    pub exhausted_nodes: Vec<String>,
    pub pressure_notes: Vec<PressureItem>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CostedExample {
    pub example_id: String,
    pub approach: ActionVector,
    pub example: String,
    pub likely_check: Option<String>,
    pub success_hint: String,
    pub failure_or_cost_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GuidanceDecision {
    pub level: GuidanceLevel,
    pub prompt_kind: PlayerDecisionPromptKind,
    pub reason: String,
    pub forbid_single_correct_path: bool,
    pub min_costed_examples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq)]
pub struct NpcBiasedAdvice {
    pub npc_id: String,
    pub speaker_label: String,
    pub advice_text: String,
    pub bias_or_goal: String,
    pub not_official_solution: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ClockTick {
    pub clock_id: String,
    pub label: String,
    pub previous: i32,
    pub current: i32,
    pub max: i32,
    pub reason: String,
    pub visible_to_players: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ConsequenceContract {
    pub consequence_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub on_success: String,
    pub on_failure: String,
    pub fail_forward: bool,
    pub cost_options: Vec<String>,
    pub clue_safety_policy: String,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SpotlightState {
    pub player_id: String,
    pub character_id: Option<String>,
    pub last_spotlight_turn: Option<String>,
    pub spotlight_count: u32,
    pub character_strengths: Vec<String>,
    pub preferred_playstyle: Vec<String>,
    pub pending_personal_hook: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SceneFramePurpose {
    pub scene_id: Option<String>,
    pub scene_purpose: ScenePurpose,
    pub enter_condition: Option<String>,
    pub progress_signals: Vec<String>,
    pub exit_conditions: Vec<String>,
    pub fail_forward_options: Vec<String>,
    pub pacing_budget_turns: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActionableSituationBrief {
    pub brief_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub where_are_we: String,
    pub visible_facts: Vec<VisibleFact>,
    pub pressure: Vec<PressureItem>,
    pub affordances: Vec<Affordance>,
    pub risks: Vec<RiskItem>,
    pub known_facts: Vec<KnownFact>,
    pub open_questions: Vec<OpenQuestion>,
    pub guidance: GuidanceDecision,
    pub goal_prompt: String,
    pub costed_examples: Vec<CostedExample>,
    pub npc_advice: Vec<NpcBiasedAdvice>,
    #[serde(default)]
    pub fresh_change: Option<FreshChange>,
    pub scene_purpose: Option<SceneFramePurpose>,
    pub avoid_single_correct_path: bool,
    pub player_facing: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DirectorTurnResult {
    pub handled: bool,
    pub phases: Vec<String>,
    pub brief: Option<ActionableSituationBrief>,
    pub clue_board: Option<PlayerFacingClueBoard>,
    pub consequence: Option<ConsequenceContract>,
    pub clock_ticks: Vec<ClockTick>,
    pub spotlight: Vec<SpotlightState>,
    #[serde(default)]
    pub novelty: Option<NoveltyDecision>,
    pub narration_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuntimeState {
    #[serde(default)]
    pub world_time: Option<WorldTimeState>,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub chapter_id: Option<String>,
    pub mission_id: Option<String>,
    pub scene_id: Option<String>,
    pub location_id: Option<String>,
    pub active_npc_ids: Vec<String>,
    pub active_material_refs: Vec<String>,
    pub pending_material_refs: Vec<String>,
    pub recent_intent_tags: Vec<String>,
    pub current_tracks: BTreeMap<String, i32>,
    pub world_state: WorldState,
    /// true ⇒ prepare_turn_context 的 BP1 用 agent-loop 版 engine protocol
    /// （GM agent loop 专用；run_gm_turn 强制置位，旧路径恒 false）。
    #[serde(default)]
    pub agent_loop_protocol: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ViewerKind {
    Gm,
    Player,
    Npc,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VisibilityProfile {
    pub viewer_kind: ViewerKind,
    pub player_id: Option<String>,
    pub actor_id: Option<String>,
    pub can_see_gm_only: bool,
}

impl VisibilityProfile {
    pub fn gm() -> Self {
        Self { viewer_kind: ViewerKind::Gm, player_id: None, actor_id: None, can_see_gm_only: true }
    }
    pub fn player(player_id: impl Into<String>, actor_id: impl Into<String>) -> Self {
        Self { viewer_kind: ViewerKind::Player, player_id: Some(player_id.into()), actor_id: Some(actor_id.into()), can_see_gm_only: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TokenBudget {
    pub prefix_max: u32,
    pub pinned_max: u32,
    pub dynamic_max: u32,
    pub total_max: u32,
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self { prefix_max: 48_000, pinned_max: 32_000, dynamic_max: 12_000, total_max: 96_000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContextRequest {
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub session_id: String,
    pub turn_id: String,
    pub viewer: VisibilityProfile,
    pub token_budget: TokenBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CompiledContext {
    pub prefix_blocks: Vec<ContextBlock>,
    pub pinned_blocks: Vec<ContextBlock>,
    pub dynamic_blocks: Vec<ContextBlock>,
    pub prefix_text: String,
    pub pinned_text: String,
    pub dynamic_text: String,
    pub prefix_hash: String,
    pub pinned_hash: String,
    pub dynamic_hash: String,
    pub visibility_signature: String,
    pub cache_key: String,
    pub token_estimate: u32,
    pub block_version_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PromptBands {
    pub system_prefix: String,
    pub semi_stable_context: String,
    pub dynamic_tail: String,
    pub history: Vec<ChatMessage>,
    pub user_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// -----------------------------------------------------------------------------
// Unified search / Tantivy retrieval model
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    Auto,
    Query,
    Phrase,
    Literal,
    Fuzzy,
}

impl Default for SearchMode {
    fn default() -> Self { Self::Auto }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchDocument {
    /// Stable unique key inside the Tantivy index. It should include origin and source primary key.
    pub search_doc_id: String,
    /// Open string instead of enum so new source types do not require model migrations.
    pub origin: String,
    /// High-level domain, e.g. rules, modules, memory, rulings, learned, source, parsed.
    pub domain: String,
    /// Open string for rule/module/memory kind, e.g. procedure, scene_node, npc, memory_event.
    pub logical_kind: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    /// Generic scope map. Examples: ruleset_id, module_id, session_id, scene_id, bundle_id, scope_type.
    pub scopes: BTreeMap<String, String>,
    pub visibility: Visibility,
    pub stability: Stability,
    pub source_refs: Vec<SourceRef>,
    pub metadata: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

impl SearchDocument {
    pub fn facets(&self) -> Vec<String> {
        let mut facets = Vec::new();
        facets.push(format!("origin__{}", normalize_facet_value(&self.origin)));
        facets.push(format!("domain__{}", normalize_facet_value(&self.domain)));
        facets.push(format!("kind__{}", normalize_facet_value(&self.logical_kind)));
        facets.push(format!("visibility__{}", self.visibility.as_str()));
        facets.push(format!("stability__{}", self.stability.as_str()));
        for tag in &self.tags {
            facets.push(format!("tag__{}", normalize_facet_value(tag)));
        }
        for (key, value) in &self.scopes {
            if !value.is_empty() {
                facets.push(format!("{}__{}", normalize_facet_value(key), normalize_facet_value(value)));
            }
        }
        facets.sort();
        facets.dedup();
        facets
    }
}

pub fn normalize_facet_value(input: &str) -> String {
    input
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub scopes: BTreeMap<String, String>,
    /// Generic exact-match filters against origin/domain/kind/tag/scope/metadata string fields.
    #[serde(default)]
    pub filters: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_search_limit")]
    pub limit: u32,
    #[serde(default)]
    pub explain: bool,
    #[serde(default = "default_search_viewer")]
    pub viewer: VisibilityProfile,
    #[serde(default = "default_search_rewrite_query")]
    pub rewrite_query: bool,
    #[serde(default)]
    pub intent: Option<String>,
}

fn default_search_limit() -> u32 { 10 }
fn default_search_viewer() -> VisibilityProfile { VisibilityProfile::gm() }
fn default_search_rewrite_query() -> bool { true }

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::Auto,
            domains: vec![],
            kinds: vec![],
            tags: vec![],
            scopes: BTreeMap::new(),
            filters: BTreeMap::new(),
            limit: default_search_limit(),
            explain: false,
            viewer: VisibilityProfile::gm(),
            rewrite_query: true,
            intent: None,
        }
    }
}

impl SearchRequest {
    pub fn effective_limit(&self) -> usize { self.limit.clamp(1, 100) as usize }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SearchHit {
    pub hit_id: String,
    pub search_doc_id: String,
    pub origin: String,
    pub domain: String,
    pub logical_kind: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
    pub scopes: BTreeMap<String, String>,
    pub tags: Vec<String>,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub explain: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SearchResponse {
    pub query_id: String,
    pub hits: Vec<SearchHit>,
    pub total_considered: usize,
    pub index_generation: Option<String>,
    #[serde(default)]
    pub rewritten_query: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SearchIndexStats {
    pub indexed_documents: usize,
    pub updated_documents: usize,
    pub deleted_documents: usize,
    pub skipped_documents: usize,
    pub skipped_sources: usize,
    pub skipped_unchanged_documents: usize,
    pub source_count: usize,
    pub errors: Vec<String>,
    pub index_dir: String,
    pub incremental: bool,
    pub mode: String,
    pub watermark: Option<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchLoadRequest {
    pub hit: SearchHit,
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub scene_id: Option<String>,
    #[serde(default)]
    pub ruleset_id: Option<String>,
    #[serde(default)]
    pub module_id: Option<String>,
    #[serde(default)]
    pub demand_id: Option<String>,
    #[serde(default)]
    pub query_text: Option<String>,
    pub cache_zone: CacheZone,
    /// Supported values: turn, scene, session, none. Unknown values are treated as turn.
    pub ttl: String,
    pub load_reason: String,
    #[serde(default = "default_true")]
    pub persist: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchLoadResponse {
    pub block: ContextBlock,
    pub persisted: bool,
    pub lookup_event_id: Option<String>,
}

impl SearchLoadRequest {
    pub fn to_context_block(&self) -> ContextBlock {
        let ttl = normalize_search_ttl(&self.ttl);
        let session_or_global = self.session_id.clone().unwrap_or_else(|| "global".into());
        let scene_id_for_seed = self.scene_id.clone().or_else(|| self.hit.scopes.get("scene_id").cloned());
        let stable_seed = match ttl.as_str() {
            "scene" => serde_json::json!({
                "search_doc_id": &self.hit.search_doc_id,
                "session_id": &self.session_id,
                "scene_id": &scene_id_for_seed,
                "cache_zone": self.cache_zone.as_str(),
                "ttl": &ttl,
            }),
            "session" | "none" => serde_json::json!({
                "search_doc_id": &self.hit.search_doc_id,
                "session_id": &self.session_id,
                "cache_zone": self.cache_zone.as_str(),
                "ttl": &ttl,
            }),
            _ => serde_json::json!({
                "search_doc_id": &self.hit.search_doc_id,
                "session_id": &self.session_id,
                "turn_id": &self.turn_id,
                "cache_zone": self.cache_zone.as_str(),
                "ttl": &ttl,
            }),
        };
        let suffix = stable_json_hash(&stable_seed).replace("sha256:", "").chars().take(24).collect::<String>();
        let (scope, stability) = match (self.cache_zone, ttl.as_str()) {
            (CacheZone::PinnedMiddle, "scene") => (
                Scope {
                    scope_type: ScopeType::Scene,
                    scope_id: self.scene_id.clone()
                        .or_else(|| self.hit.scopes.get("scene_id").cloned())
                        .unwrap_or_else(|| self.session_id.clone().unwrap_or_else(|| "scene_unknown".into())),
                },
                Stability::SceneStable,
            ),
            (CacheZone::PinnedMiddle, "session") => (
                Scope { scope_type: ScopeType::Session, scope_id: self.session_id.clone().unwrap_or_else(|| "session_unknown".into()) },
                Stability::SceneStable,
            ),
            (CacheZone::Prefix, _) => (
                self.ruleset_id.as_ref().map(|id| Scope::ruleset(id.clone())).unwrap_or_else(Scope::global),
                Stability::RarelyChanged,
            ),
            (_, "none") => (
                Scope { scope_type: ScopeType::Session, scope_id: session_or_global.clone() },
                Stability::Ephemeral,
            ),
            _ => (
                Scope { scope_type: ScopeType::Turn, scope_id: self.turn_id.clone().unwrap_or_else(|| session_or_global.clone()) },
                Stability::TurnDynamic,
            ),
        };

        let source_refs = serde_json::to_string(&self.hit.source_refs).unwrap_or_default();
        let (display_title, display_snippet, redaction_note) = player_facing_safe_search_text(&self.hit);
        let mut block = ContextBlock::new(
            format!("runtime.search.{}.{}", session_or_global, suffix),
            BlockKind::LookupResult,
            format!("Search result: {}", display_title),
            BlockContent::Markdown(format!(
                "# {}

Domain: `{}`  Kind: `{}`  Origin: `{}`  Score: {:.3}

{}{}

Source refs: `{}`",
                display_title,
                self.hit.domain,
                self.hit.logical_kind,
                self.hit.origin,
                self.hit.score,
                redaction_note,
                display_snippet,
                source_refs
            )),
            self.hit.visibility,
            stability,
            self.cache_zone,
            scope,
            if self.cache_zone == CacheZone::PinnedMiddle { 88 } else { 116 },
        );
        block.tags = vec![
            "search_result".into(),
            "runtime_loaded_search".into(),
            format!("ttl_{}", ttl),
            self.hit.domain.clone(),
            self.hit.logical_kind.clone(),
        ];
        block.source_refs = self.hit.source_refs.clone();
        block.load_reason = Some(self.load_reason.clone());
        if ttl == "turn" {
            block.expires_at_turn = Some(self.turn_id.clone().unwrap_or_else(|| "turn_unspecified".into()));
        }
        if ttl == "scene" {
            block.expires_at_scene = Some(self.scene_id.clone().or_else(|| self.hit.scopes.get("scene_id").cloned()).unwrap_or_else(|| "scene_unspecified".into()));
        }
        block
    }
}


fn player_facing_safe_search_text(hit: &SearchHit) -> (String, String, String) {
    let redact = std::env::var("TRPG_REDACT_GM_ONLY_SEARCH_HITS")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(true);
    if !redact || hit.visibility != Visibility::GmOnly {
        return (hit.title.clone(), hit.snippet.clone(), String::new());
    }

    // Runtime LLM output is player-facing by default. GM-only search hits can be
    // useful for adjudication, but exact hidden names must not be shown to the
    // model as ordinary player-facing vocabulary. Hidden terms can be supplied by
    // parser metadata or by environment override; the default covers the first
    // Homecoming regression without hard-coding the whole module parser.
    let mut secret_terms = Vec::new();
    if let Some(arr) = hit.metadata.get("secret_terms").and_then(|v| v.as_array()) {
        for term in arr.iter().filter_map(|v| v.as_str()) {
            if !term.trim().is_empty() {
                secret_terms.push(term.trim().to_string());
            }
        }
    }
    if let Ok(env_terms) = std::env::var("TRPG_SECRET_TERM_OVERRIDES") {
        for term in env_terms.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            secret_terms.push(term.to_string());
        }
    } else {
        secret_terms.extend(["Athena".to_string(), "Shelob".to_string()]);
    }
    if hit.title.contains("Athena") || hit.snippet.contains("Athena") {
        secret_terms.push("Athena".to_string());
    }
    secret_terms.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secret_terms.dedup();

    let mut title = hit.title.clone();
    let mut snippet = hit.snippet.clone();
    for term in &secret_terms {
        title = replace_case_sensitive_term(&title, term, "[undiscovered entity]");
        snippet = replace_case_sensitive_term(&snippet, term, "[undiscovered entity]");
    }
    let note = "GM-only lookup loaded for adjudication. Do not reveal redacted names or infer exact hidden identities unless the player has learned them in-fiction. ".to_string();
    (title, snippet, note)
}

fn replace_case_sensitive_term(input: &str, term: &str, replacement: &str) -> String {
    if term.is_empty() || !input.contains(term) {
        return input.to_string();
    }
    input.replace(term, replacement)
}

pub fn normalize_search_ttl(ttl: &str) -> String {
    match ttl.trim().to_ascii_lowercase().as_str() {
        "scene" => "scene".to_string(),
        "session" => "session".to_string(),
        "none" | "manual" | "permanent" => "none".to_string(),
        _ => "turn".to_string(),
    }
}


// -----------------------------------------------------------------------------
// Semantic Rule Binding & Ability Hydration Kernel (v1.9)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindingStatus {
    Unbound,
    NamesOnly,
    HydrationRequested,
    BoundProvisional,
    BoundExact,
    NeedsReview,
    FailedNoSource,
    FailedConflict,
}
impl Default for BindingStatus { fn default() -> Self { Self::Unbound } }
impl BindingStatus { pub fn as_str(&self) -> &'static str { match self { Self::Unbound => "unbound", Self::NamesOnly => "names_only", Self::HydrationRequested => "hydration_requested", Self::BoundProvisional => "bound_provisional", Self::BoundExact => "bound_exact", Self::NeedsReview => "needs_review", Self::FailedNoSource => "failed_no_source", Self::FailedConflict => "failed_conflict" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleBindingTargetKind {
    ActorParameter,
    ObjectDefinition,
    AbilityDefinition,
    CheckContract,
    EffectContract,
    ContestProfile,
    ConditionDefinition,
    Unknown,
}
impl Default for RuleBindingTargetKind { fn default() -> Self { Self::Unknown } }
impl RuleBindingTargetKind { pub fn as_str(&self) -> &'static str { match self { Self::ActorParameter => "actor_parameter", Self::ObjectDefinition => "object_definition", Self::AbilityDefinition => "ability_definition", Self::CheckContract => "check_contract", Self::EffectContract => "effect_contract", Self::ContestProfile => "contest_profile", Self::ConditionDefinition => "condition_definition", Self::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeUrgency { Immediate, BeforeResolution, Soon, Background }
impl Default for RuntimeUrgency { fn default() -> Self { Self::Soon } }
impl RuntimeUrgency { pub fn as_str(&self) -> &'static str { match self { Self::Immediate => "immediate", Self::BeforeResolution => "before_resolution", Self::Soon => "soon", Self::Background => "background" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MaterializationRequest {
    pub request_id: String,
    pub target_kind: RuleBindingTargetKind,
    pub target_id: Option<String>,
    pub target_label: String,
    pub target_description: String,
    pub evidence_span: String,
    pub requested_fields: Vec<String>,
    pub urgency: RuntimeUrgency,
    pub visibility: Visibility,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SemanticIntentRequest {
    pub session_id: String,
    pub turn_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub active_frame_summary: serde_json::Value,
    pub active_gate_summary: serde_json::Value,
    pub player_input: String,
    pub visible_scene_objects: serde_json::Value,
    pub actor_parameter_summary: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SemanticIntentResult {
    pub semantic_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub ruleset_id: String,
    pub primary_action_kind: SituationActionKind,
    pub gate_relation: String,
    pub frame_relation: FrameRelation,
    pub target_refs: Vec<String>,
    pub object_refs: Vec<String>,
    pub ability_refs: Vec<String>,
    pub materialization_requests: Vec<MaterializationRequest>,
    pub secrecy_policy: String,
    pub confidence: RulingConfidence,
    pub classifier: String,
    pub rationale_brief: Option<String>,
    pub raw_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SemanticQueryPlan {
    pub plan_id: String,
    pub target_kind: RuleBindingTargetKind,
    pub target_label: String,
    pub ruleset_id: String,
    pub exact_name_queries: Vec<String>,
    pub alias_queries: Vec<String>,
    pub field_queries: Vec<String>,
    pub broad_queries: Vec<String>,
    pub requested_fields: Vec<String>,
    pub query_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuleBindingPacket {
    pub binding_id: String,
    pub target_kind: RuleBindingTargetKind,
    pub target_id: String,
    pub ruleset_id: String,
    pub semantic_request_json: serde_json::Value,
    pub retrieval_queries: Vec<String>,
    pub source_hits: Vec<SearchHit>,
    pub extracted_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub confidence: RulingConfidence,
    pub verification_status: BindingStatus,
    pub created_at_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}


// -----------------------------------------------------------------------------
// Real Materialization Extractor & Binding Verifier (v1.10)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialTargetKind {
    ActorProfile,
    ObjectDefinition,
    ObjectInstance,
    AbilityDefinition,
    AbilityInstance,
    NpcStatBlock,
    VehicleCard,
    EncounterCard,
    CheckTarget,
    EffectProfile,
    DamageProfile,
    ArmorProfile,
    ConditionDefinition,
    Unknown,
}
impl Default for MaterialTargetKind { fn default() -> Self { Self::Unknown } }
impl MaterialTargetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ActorProfile => "actor_profile",
            Self::ObjectDefinition => "object_definition",
            Self::ObjectInstance => "object_instance",
            Self::AbilityDefinition => "ability_definition",
            Self::AbilityInstance => "ability_instance",
            Self::NpcStatBlock => "npc_stat_block",
            Self::VehicleCard => "vehicle_card",
            Self::EncounterCard => "encounter_card",
            Self::CheckTarget => "check_target",
            Self::EffectProfile => "effect_profile",
            Self::DamageProfile => "damage_profile",
            Self::ArmorProfile => "armor_profile",
            Self::ConditionDefinition => "condition_definition",
            Self::Unknown => "unknown",
        }
    }
    pub fn to_rule_binding_target(self) -> RuleBindingTargetKind {
        match self {
            Self::ActorProfile | Self::NpcStatBlock | Self::VehicleCard | Self::EncounterCard => RuleBindingTargetKind::ActorParameter,
            Self::ObjectDefinition | Self::ObjectInstance | Self::ArmorProfile | Self::DamageProfile => RuleBindingTargetKind::ObjectDefinition,
            Self::AbilityDefinition | Self::AbilityInstance => RuleBindingTargetKind::AbilityDefinition,
            Self::CheckTarget => RuleBindingTargetKind::CheckContract,
            Self::EffectProfile => RuleBindingTargetKind::EffectContract,
            Self::ConditionDefinition => RuleBindingTargetKind::ConditionDefinition,
            Self::Unknown => RuleBindingTargetKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationUrgency {
    BlockingMechanicalResolution,
    BeforeNarration,
    BackgroundPrewarm,
    AuditOnly,
}
impl Default for MaterializationUrgency { fn default() -> Self { Self::BeforeNarration } }
impl MaterializationUrgency { pub fn as_str(&self) -> &'static str { match self { Self::BlockingMechanicalResolution => "blocking_mechanical_resolution", Self::BeforeNarration => "before_narration", Self::BackgroundPrewarm => "background_prewarm", Self::AuditOnly => "audit_only" } } }


// -----------------------------------------------------------------------------
// Mechanics Search Skills & Parameter Facet Bindings (v1.12)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchSkillKind {
    CombatResolution,
    WeaponParameter,
    ArmorDefense,
    AbilityActivation,
    ConditionResource,
    NpcStatblock,
    ModuleCard,
    SceneObject,
    GenericMechanical,
}
impl Default for SearchSkillKind { fn default() -> Self { Self::GenericMechanical } }
impl SearchSkillKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CombatResolution => "combat_resolution_search",
            Self::WeaponParameter => "weapon_parameter_search",
            Self::ArmorDefense => "armor_defense_search",
            Self::AbilityActivation => "ability_activation_search",
            Self::ConditionResource => "condition_resource_search",
            Self::NpcStatblock => "npc_statblock_search",
            Self::ModuleCard => "module_card_search",
            Self::SceneObject => "scene_object_search",
            Self::GenericMechanical => "generic_mechanical_search",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryPlanStepKind {
    Locator,
    ExactEntity,
    Field,
    Procedure,
    ModuleCard,
    BroadFallback,
    SemanticRerank,
    Extractor,
    RuntimeWriteback,
}
impl Default for QueryPlanStepKind { fn default() -> Self { Self::Locator } }
impl QueryPlanStepKind { pub fn as_str(&self) -> &'static str { match self { Self::Locator => "locator", Self::ExactEntity => "exact_entity", Self::Field => "field", Self::Procedure => "procedure", Self::ModuleCard => "module_card", Self::BroadFallback => "broad_fallback", Self::SemanticRerank => "semantic_rerank", Self::Extractor => "extractor", Self::RuntimeWriteback => "runtime_writeback" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterFacetKind {
    ActorCheckFacet,
    ActorDefenseFacet,
    ActorResourceTrack,
    ActorConditionTrack,
    ObjectAttackFacet,
    ObjectDamageFacet,
    ObjectArmorFacet,
    ObjectDurabilityFacet,
    AbilityActivationFacet,
    AbilityTriggerFacet,
    AbilityCostFacet,
    AbilityEffectFacet,
    CheckResolutionBinding,
    ContestResolutionBinding,
    EffectDamageBinding,
    EffectResourceBinding,
    EffectConditionBinding,
    VisibilityBinding,
    Unknown,
}
impl Default for ParameterFacetKind { fn default() -> Self { Self::Unknown } }
impl ParameterFacetKind { pub fn as_str(&self) -> &'static str { match self { Self::ActorCheckFacet => "actor_check_facet", Self::ActorDefenseFacet => "actor_defense_facet", Self::ActorResourceTrack => "actor_resource_track", Self::ActorConditionTrack => "actor_condition_track", Self::ObjectAttackFacet => "object_attack_facet", Self::ObjectDamageFacet => "object_damage_facet", Self::ObjectArmorFacet => "object_armor_facet", Self::ObjectDurabilityFacet => "object_durability_facet", Self::AbilityActivationFacet => "ability_activation_facet", Self::AbilityTriggerFacet => "ability_trigger_facet", Self::AbilityCostFacet => "ability_cost_facet", Self::AbilityEffectFacet => "ability_effect_facet", Self::CheckResolutionBinding => "check_resolution_binding", Self::ContestResolutionBinding => "contest_resolution_binding", Self::EffectDamageBinding => "effect_damage_binding", Self::EffectResourceBinding => "effect_resource_binding", Self::EffectConditionBinding => "effect_condition_binding", Self::VisibilityBinding => "visibility_binding", Self::Unknown => "unknown" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SearchSkillProfile {
    pub skill_id: String,
    pub skill_kind: SearchSkillKind,
    pub demand_kinds: Vec<MaterialTargetKind>,
    pub source_priority: Vec<SourceKind>,
    pub corpus_filters: Vec<String>,
    pub required_fields: Vec<String>,
    pub optional_fields: Vec<String>,
    pub extractor_schema: ExtractorKind,
    pub verifier_policy: String,
    pub writeback_targets: Vec<RuleBindingTargetKind>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct QueryPlanStep {
    pub step_kind: QueryPlanStepKind,
    pub queries: Vec<String>,
    pub purpose: String,
    pub stop_when: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MechanicsQueryPlan {
    pub plan_id: String,
    pub demand_id: Option<String>,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub skill_kind: SearchSkillKind,
    pub target_kind: MaterialTargetKind,
    pub target_label: String,
    pub requested_fields: Vec<String>,
    pub steps: Vec<QueryPlanStep>,
    pub ruleset_aliases: serde_json::Value,
    pub module_source_preferences: Vec<String>,
    pub writeback_targets: Vec<RuleBindingTargetKind>,
    pub created_at_tick: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ParameterFacetBinding {
    pub facet_binding_id: String,
    pub session_id: String,
    pub target_kind: RuleBindingTargetKind,
    pub target_id: String,
    pub facet_kind: ParameterFacetKind,
    pub binding_id: String,
    pub demand_id: Option<String>,
    pub facet_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub confidence: RulingConfidence,
    pub verification_status: BindingStatus,
    pub world_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MechanicsSearchTrace {
    pub trace_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub demand_id: Option<String>,
    pub skill_kind: SearchSkillKind,
    pub plan: MechanicsQueryPlan,
    pub result_summary: serde_json::Value,
    pub created_at: DateTime<Utc>,
}


// -----------------------------------------------------------------------------
// Parameter Facet Executor data contracts (v1.13)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FacetExecutionStatus {
    Applied,
    AppliedProvisional,
    NoMatchingFacet,
    RejectedByValidator,
    DeferredNeedsBinding,
}
impl Default for FacetExecutionStatus { fn default() -> Self { Self::DeferredNeedsBinding } }
impl FacetExecutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::AppliedProvisional => "applied_provisional",
            Self::NoMatchingFacet => "no_matching_facet",
            Self::RejectedByValidator => "rejected_by_validator",
            Self::DeferredNeedsBinding => "deferred_needs_binding",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FacetExecutionKind {
    EffectImpact,
    ContestResolution,
    DamageReduction,
    ResourceDelta,
    ConditionApply,
    ObjectStatePatch,
    VisibilityRedaction,
    TableLookup,
}
impl Default for FacetExecutionKind { fn default() -> Self { Self::EffectImpact } }
impl FacetExecutionKind { pub fn as_str(&self) -> &'static str { match self { Self::EffectImpact => "effect_impact", Self::ContestResolution => "contest_resolution", Self::DamageReduction => "damage_reduction", Self::ResourceDelta => "resource_delta", Self::ConditionApply => "condition_apply", Self::ObjectStatePatch => "object_state_patch", Self::VisibilityRedaction => "visibility_redaction", Self::TableLookup => "table_lookup" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ParameterFacetExecution {
    pub execution_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub frame_id: Option<String>,
    pub execution_kind: FacetExecutionKind,
    pub source_facet_binding_ids: Vec<String>,
    pub ruleset_id: String,
    pub target_kind: EffectTargetKind,
    pub target_id: String,
    pub parameter_path: String,
    pub operation: ParameterOperation,
    pub input_json: serde_json::Value,
    pub output_json: serde_json::Value,
    pub status: FacetExecutionStatus,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
    pub world_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct GenericParameterState {
    pub state_id: String,
    pub session_id: String,
    pub target_kind: EffectTargetKind,
    pub target_id: String,
    pub parameter_path: String,
    pub value: serde_json::Value,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
    pub world_tick: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RulesetMechanicalProfile {
    pub profile_id: String,
    pub ruleset_id: String,
    pub profile_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationDemandStatus {
    Open,
    EvidenceCollected,
    Extracted,
    Bound,
    Verified,
    Provisional,
    Failed,
    Superseded,
}
impl Default for MaterializationDemandStatus { fn default() -> Self { Self::Open } }
impl MaterializationDemandStatus { pub fn as_str(&self) -> &'static str { match self { Self::Open => "open", Self::EvidenceCollected => "evidence_collected", Self::Extracted => "extracted", Self::Bound => "bound", Self::Verified => "verified", Self::Provisional => "provisional", Self::Failed => "failed", Self::Superseded => "superseded" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct MaterializationDemand {
    pub demand_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub frame_id: Option<String>,
    pub target_kind: MaterialTargetKind,
    pub target_id: Option<String>,
    pub target_label: String,
    pub target_description: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub requested_fields: Vec<String>,
    pub urgency: MaterializationUrgency,
    pub evidence_json: serde_json::Value,
    pub visibility: Visibility,
    pub caused_by_event_id: Option<String>,
    pub world_tick: Option<i64>,
    pub status: MaterializationDemandStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SourceCandidate {
    pub candidate_id: String,
    pub source_document_id: String,
    pub source_kind: SourceKind,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub heading_path: Option<String>,
    pub excerpt: String,
    pub excerpt_hash: String,
    pub candidate_score: f32,
    pub retrieval_reason: String,
    pub source_refs: Vec<SourceRef>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SourceEvidenceBundle {
    pub bundle_id: String,
    pub demand_id: String,
    pub candidates: Vec<SourceCandidate>,
    pub query_plan_json: serde_json::Value,
    pub source_priority_order: Vec<SourceKind>,
    pub created_at_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtractorKind { ActorProfile, ObjectEntry, AbilityEntry, NpcCard, VehicleCard, DamageProfile, ArmorProfile, CheckProcedure, Condition, Generic }
impl Default for ExtractorKind { fn default() -> Self { Self::Generic } }
impl ExtractorKind { pub fn as_str(&self) -> &'static str { match self { Self::ActorProfile => "actor_profile", Self::ObjectEntry => "object_entry", Self::AbilityEntry => "ability_entry", Self::NpcCard => "npc_card", Self::VehicleCard => "vehicle_card", Self::DamageProfile => "damage_profile", Self::ArmorProfile => "armor_profile", Self::CheckProcedure => "check_procedure", Self::Condition => "condition", Self::Generic => "generic" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ExtractionRun {
    pub extraction_run_id: String,
    pub demand_id: String,
    pub extractor_kind: ExtractorKind,
    pub input_candidate_ids: Vec<String>,
    pub extracted_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub confidence: RulingConfidence,
    pub status: BindingStatus,
    pub model_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindingVerificationStatus {
    VerifiedExact,
    VerifiedPartial,
    ProvisionalNeedsAudit,
    RejectedNoSource,
    RejectedContradictory,
    RejectedVisibilityLeak,
    RejectedSchemaInvalid,
}
impl Default for BindingVerificationStatus { fn default() -> Self { Self::ProvisionalNeedsAudit } }
impl BindingVerificationStatus { pub fn as_str(&self) -> &'static str { match self { Self::VerifiedExact => "verified_exact", Self::VerifiedPartial => "verified_partial", Self::ProvisionalNeedsAudit => "provisional_needs_audit", Self::RejectedNoSource => "rejected_no_source", Self::RejectedContradictory => "rejected_contradictory", Self::RejectedVisibilityLeak => "rejected_visibility_leak", Self::RejectedSchemaInvalid => "rejected_schema_invalid" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct BindingVerificationResult {
    pub verification_id: String,
    pub binding_id: String,
    pub target_kind: RuleBindingTargetKind,
    pub target_id: String,
    pub status: BindingVerificationStatus,
    pub missing_required_fields: Vec<String>,
    pub contradictory_sources: Vec<String>,
    pub visibility_issues: Vec<String>,
    pub confidence_adjustment: Option<RulingConfidence>,
    pub repair_suggestions: Vec<String>,
    pub verifier_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RuntimeBindingWriteback {
    pub writeback_id: String,
    pub binding_id: String,
    pub target_kind: RuleBindingTargetKind,
    pub target_id: String,
    pub table_name: String,
    pub writeback_json: serde_json::Value,
    pub status: String,
    pub created_at_tick: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilityKind {
    Spell,
    Cantrip,
    Ritual,
    CombatFeat,
    RoleAbility,
    SkillUse,
    Technique,
    Stunt,
    Evocation,
    Program,
    NetrunAction,
    Reaction,
    PassiveTrait,
    ClassFeature,
    MonsterAction,
    LegendaryAction,
    LairAction,
    AnomalyEffect,
    Requisition,
    ConditionGrantedAbility,
    ObjectGrantedAbility,
    Unknown,
}
impl Default for AbilityKind { fn default() -> Self { Self::Unknown } }
impl AbilityKind { pub fn as_str(&self) -> &'static str { match self { Self::Spell => "spell", Self::Cantrip => "cantrip", Self::Ritual => "ritual", Self::CombatFeat => "combat_feat", Self::RoleAbility => "role_ability", Self::SkillUse => "skill_use", Self::Technique => "technique", Self::Stunt => "stunt", Self::Evocation => "evocation", Self::Program => "program", Self::NetrunAction => "netrun_action", Self::Reaction => "reaction", Self::PassiveTrait => "passive_trait", Self::ClassFeature => "class_feature", Self::MonsterAction => "monster_action", Self::LegendaryAction => "legendary_action", Self::LairAction => "lair_action", Self::AnomalyEffect => "anomaly_effect", Self::Requisition => "requisition", Self::ConditionGrantedAbility => "condition_granted_ability", Self::ObjectGrantedAbility => "object_granted_ability", Self::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilitySourceKind { ActorClass, ActorSkill, SpeciesOrRace, ObjectGranted, StatusGranted, ModuleNpcCard, MonsterStatBlock, RulebookEntry, Anomaly, Unknown }
impl Default for AbilitySourceKind { fn default() -> Self { Self::Unknown } }
impl AbilitySourceKind { pub fn as_str(&self) -> &'static str { match self { Self::ActorClass => "actor_class", Self::ActorSkill => "actor_skill", Self::SpeciesOrRace => "species_or_race", Self::ObjectGranted => "object_granted", Self::StatusGranted => "status_granted", Self::ModuleNpcCard => "module_npc_card", Self::MonsterStatBlock => "monster_stat_block", Self::RulebookEntry => "rulebook_entry", Self::Anomaly => "anomaly", Self::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilityTriggerKind { OnActionDeclared, OnAttackDeclared, OnHit, OnMiss, OnDamageRoll, OnTakeDamage, OnDefense, OnFailedCheck, OnSuccessfulCheck, OnObjectInteraction, OnMovement, OnSceneStart, OnTurnStart, OnTurnEnd, OnRoundStart, OnWorldTimeTick, OnRest, OnDeathOrDying, OnNpcAttitudeChange, ManualActivation, PassiveAlways }
impl Default for AbilityTriggerKind { fn default() -> Self { Self::ManualActivation } }
impl AbilityTriggerKind { pub fn as_str(&self) -> &'static str { match self { Self::OnActionDeclared => "on_action_declared", Self::OnAttackDeclared => "on_attack_declared", Self::OnHit => "on_hit", Self::OnMiss => "on_miss", Self::OnDamageRoll => "on_damage_roll", Self::OnTakeDamage => "on_take_damage", Self::OnDefense => "on_defense", Self::OnFailedCheck => "on_failed_check", Self::OnSuccessfulCheck => "on_successful_check", Self::OnObjectInteraction => "on_object_interaction", Self::OnMovement => "on_movement", Self::OnSceneStart => "on_scene_start", Self::OnTurnStart => "on_turn_start", Self::OnTurnEnd => "on_turn_end", Self::OnRoundStart => "on_round_start", Self::OnWorldTimeTick => "on_world_time_tick", Self::OnRest => "on_rest", Self::OnDeathOrDying => "on_death_or_dying", Self::OnNpcAttitudeChange => "on_npc_attitude_change", Self::ManualActivation => "manual_activation", Self::PassiveAlways => "passive_always" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityActivationModel { pub activation_kind: String, pub action_cost: Option<String>, pub timing: Option<String>, pub notes: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityCostModel { pub resource: Option<String>, pub amount_json: serde_json::Value, pub consumes_use: bool, pub notes: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityTargetModel { pub target_kind: String, pub range: Option<String>, pub area: Option<String>, pub notes: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityEffectModel { pub effect_kind: String, pub summary: String, pub creates_check: bool, pub creates_effect: bool, pub duration: Option<String>, pub raw_json: serde_json::Value }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityRuleBinding {
    pub binding_id: String,
    pub trigger: AbilityTriggerKind,
    pub rule_packet_id: Option<String>,
    pub lookup_recipe_id: Option<String>,
    pub source_refs: Vec<SourceRef>,
    pub effect_json: serde_json::Value,
    pub confidence: RulingConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityDefinition {
    pub ability_def_id: String,
    pub ruleset_id: String,
    pub name: String,
    pub ability_kind: AbilityKind,
    pub tags: Vec<String>,
    pub source_kind: AbilitySourceKind,
    pub source_refs: Vec<SourceRef>,
    pub activation: AbilityActivationModel,
    pub cost: AbilityCostModel,
    pub target: AbilityTargetModel,
    pub effect: AbilityEffectModel,
    pub rule_bindings: Vec<AbilityRuleBinding>,
    pub visibility_default: Visibility,
    pub mechanical_profile: serde_json::Value,
    pub binding_status: BindingStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilityKnownState { Unknown, Suspected, KnownByActor, KnownByTable, HiddenGmOnly, Playwalled }
impl Default for AbilityKnownState { fn default() -> Self { Self::KnownByActor } }
impl AbilityKnownState { pub fn as_str(&self) -> &'static str { match self { Self::Unknown => "unknown", Self::Suspected => "suspected", Self::KnownByActor => "known_by_actor", Self::KnownByTable => "known_by_table", Self::HiddenGmOnly => "hidden_gm_only", Self::Playwalled => "playwalled" } } }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilityPreparedState { AlwaysAvailable, Prepared, NotPrepared, Expended, Dormant }
impl Default for AbilityPreparedState { fn default() -> Self { Self::AlwaysAvailable } }
impl AbilityPreparedState { pub fn as_str(&self) -> &'static str { match self { Self::AlwaysAvailable => "always_available", Self::Prepared => "prepared", Self::NotPrepared => "not_prepared", Self::Expended => "expended", Self::Dormant => "dormant" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityUsesState { pub current: Option<i32>, pub max: Option<i32>, pub refresh: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityCooldownState { pub cooldown_until_tick: Option<i64>, pub cooldown_remaining_turns: Option<i32> }
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityVisibilityState { pub visibility: Visibility, pub known_state: AbilityKnownState, pub reveal_conditions: Vec<String> }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityInstance {
    pub ability_id: String,
    pub ability_def_id: String,
    pub session_id: String,
    pub owner_actor_id: Option<String>,
    pub granted_by_object_id: Option<String>,
    pub granted_by_status_id: Option<String>,
    pub known_state: AbilityKnownState,
    pub prepared_state: AbilityPreparedState,
    pub uses_state: AbilityUsesState,
    pub cooldown_state: AbilityCooldownState,
    pub visibility_state: AbilityVisibilityState,
    pub runtime_state: serde_json::Value,
    pub created_at_tick: Option<i64>,
    pub updated_at_tick: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityTriggerBinding {
    pub trigger_id: String,
    pub ability_def_id: String,
    pub trigger_kind: AbilityTriggerKind,
    pub condition_json: serde_json::Value,
    pub required: bool,
    pub player_choice_required: bool,
    pub gm_secret_allowed: bool,
    pub creates_gate: bool,
    pub creates_check: bool,
    pub creates_effect: bool,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilityActivationKind { Manual, Reaction, Triggered, Passive, GmSecret }
impl Default for AbilityActivationKind { fn default() -> Self { Self::Manual } }
impl AbilityActivationKind { pub fn as_str(&self) -> &'static str { match self { Self::Manual => "manual", Self::Reaction => "reaction", Self::Triggered => "triggered", Self::Passive => "passive", Self::GmSecret => "gm_secret" } } }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AbilityActivationStatus { Created, WaitingForGate, WaitingForRoll, Resolved, Superseded, Failed }
impl Default for AbilityActivationStatus { fn default() -> Self { Self::Created } }
impl AbilityActivationStatus { pub fn as_str(&self) -> &'static str { match self { Self::Created => "created", Self::WaitingForGate => "waiting_for_gate", Self::WaitingForRoll => "waiting_for_roll", Self::Resolved => "resolved", Self::Superseded => "superseded", Self::Failed => "failed" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AbilityActivationContract {
    pub activation_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub actor_id: String,
    pub ability_id: String,
    pub ability_def_id: String,
    pub target_actor_ids: Vec<String>,
    pub target_object_ids: Vec<String>,
    pub target_zone_ids: Vec<String>,
    pub activation_kind: AbilityActivationKind,
    pub check_contracts: Vec<CheckContract>,
    pub effect_contracts: Vec<EffectContract>,
    pub object_patches: Vec<ObjectPatch>,
    pub actor_patches: Vec<StatePatch>,
    pub cost_patches: Vec<StatePatch>,
    pub source_refs: Vec<SourceRef>,
    pub rule_binding_ids: Vec<String>,
    pub status: AbilityActivationStatus,
    pub created_at_tick: Option<i64>,
}


// -----------------------------------------------------------------------------
// Player-supplied value verification and table override policy (v1.10.2)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerSuppliedValueKind {
    DamageRollTotal,
    DamageExpression,
    WeaponDamage,
    DifficultyValue,
    RangeDifficulty,
    ArmorValue,
    HitPoints,
    ResourceAmount,
    AbilityCost,
    GenericParameter,
}
impl Default for PlayerSuppliedValueKind { fn default() -> Self { Self::GenericParameter } }
impl PlayerSuppliedValueKind { pub fn as_str(&self) -> &'static str { match self { Self::DamageRollTotal => "damage_roll_total", Self::DamageExpression => "damage_expression", Self::WeaponDamage => "weapon_damage", Self::DifficultyValue => "difficulty_value", Self::RangeDifficulty => "range_difficulty", Self::ArmorValue => "armor_value", Self::HitPoints => "hit_points", Self::ResourceAmount => "resource_amount", Self::AbilityCost => "ability_cost", Self::GenericParameter => "generic_parameter" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerValueVerificationStatus {
    RuleConfirmed,
    TableConsistent,
    PlausibleProvisional,
    UnreasonableNeedsWarning,
    ContradictsKnownRule,
    NeedsRuleLookup,
    NeedsClarification,
    AcceptedAsTablePreference,
}
impl Default for PlayerValueVerificationStatus { fn default() -> Self { Self::NeedsRuleLookup } }
impl PlayerValueVerificationStatus { pub fn as_str(&self) -> &'static str { match self { Self::RuleConfirmed => "rule_confirmed", Self::TableConsistent => "table_consistent", Self::PlausibleProvisional => "plausible_provisional", Self::UnreasonableNeedsWarning => "unreasonable_needs_warning", Self::ContradictsKnownRule => "contradicts_known_rule", Self::NeedsRuleLookup => "needs_rule_lookup", Self::NeedsClarification => "needs_clarification", Self::AcceptedAsTablePreference => "accepted_as_table_preference" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TableOverrideStatus { Proposed, AcceptedByTable, RejectedByGm, Superseded, AuditAfterSession }
impl Default for TableOverrideStatus { fn default() -> Self { Self::Proposed } }
impl TableOverrideStatus { pub fn as_str(&self) -> &'static str { match self { Self::Proposed => "proposed", Self::AcceptedByTable => "accepted_by_table", Self::RejectedByGm => "rejected_by_gm", Self::Superseded => "superseded", Self::AuditAfterSession => "audit_after_session" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayerSuppliedValueClaim {
    pub claim_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub value_kind: PlayerSuppliedValueKind,
    pub label: String,
    pub supplied_value_json: serde_json::Value,
    pub evidence_span: String,
    pub context_summary: String,
    pub target_actor_id: Option<String>,
    pub target_object_id: Option<String>,
    pub target_ability_id: Option<String>,
    pub source_refs: Vec<SourceRef>,
    pub visibility: Visibility,
    pub world_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayerValueVerification {
    pub verification_id: String,
    pub claim_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub status: PlayerValueVerificationStatus,
    pub canonical_value_json: serde_json::Value,
    pub acceptable_range_json: serde_json::Value,
    pub comparison_json: serde_json::Value,
    pub source_refs: Vec<SourceRef>,
    pub warning_public: Option<String>,
    pub suggestion_public: Option<String>,
    pub accepted_if_player_insists: bool,
    pub balance_risk: Option<String>,
    pub verifier_json: serde_json::Value,
    pub world_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TableOverrideAgreement {
    pub override_id: String,
    pub session_id: String,
    pub claim_id: String,
    pub verification_id: String,
    pub status: TableOverrideStatus,
    pub accepted_by: Option<String>,
    pub reason: Option<String>,
    pub warning_public: String,
    pub created_at_tick: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PlayerValueRefereeResult {
    pub handled: bool,
    pub claims: Vec<PlayerSuppliedValueClaim>,
    pub verifications: Vec<PlayerValueVerification>,
    pub table_overrides: Vec<TableOverrideAgreement>,
    pub narration_context: Option<String>,
    pub phases: Vec<String>,
}

// -----------------------------------------------------------------------------
// Referee Combat Slice data contracts (v1.10.2)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateBlockingLevel { HardBlocking, SoftBlocking, Advisory, DebugOnly }
impl Default for GateBlockingLevel { fn default() -> Self { Self::HardBlocking } }
impl GateBlockingLevel { pub fn as_str(&self) -> &'static str { match self { Self::HardBlocking => "hard_blocking", Self::SoftBlocking => "soft_blocking", Self::Advisory => "advisory", Self::DebugOnly => "debug_only" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CombatRoundActionKind { ContinuePressure, Attack, TakeCover, Move, Reload, UseObject, ActivateAbility, Withdraw, Hold, AssessRisk, Unknown }
impl Default for CombatRoundActionKind { fn default() -> Self { Self::Unknown } }
impl CombatRoundActionKind { pub fn as_str(&self) -> &'static str { match self { Self::ContinuePressure => "continue_pressure", Self::Attack => "attack", Self::TakeCover => "take_cover", Self::Move => "move", Self::Reload => "reload", Self::UseObject => "use_object", Self::ActivateAbility => "activate_ability", Self::Withdraw => "withdraw", Self::Hold => "hold", Self::AssessRisk => "assess_risk", Self::Unknown => "unknown" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ActorMechanicalState {
    pub state_id: String,
    pub session_id: String,
    pub frame_id: Option<String>,
    pub actor_id: String,
    pub actor_kind: ActorKind,
    pub hp_current: Option<i32>,
    pub hp_max: Option<i32>,
    pub armor_current: Option<i32>,
    pub wound_state: Option<String>,
    pub morale: Option<i32>,
    pub resources: serde_json::Value,
    pub conditions: Vec<serde_json::Value>,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
    pub world_tick: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatRoundAction {
    pub action_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub action_kind: CombatRoundActionKind,
    pub source_actor_id: String,
    pub target_actor_id: Option<String>,
    pub source_object_id: Option<String>,
    pub source_ability_id: Option<String>,
    pub declared_goal: String,
    pub world_tick: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AttackResolutionContract {
    pub attack_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub source_actor_id: String,
    pub target_actor_id: Option<String>,
    pub target_object_id: Option<String>,
    pub source_object_id: Option<String>,
    pub source_ability_id: Option<String>,
    pub range_band: Option<String>,
    pub cover_state: Option<String>,
    pub attack_check_id: Option<String>,
    pub contest_id: Option<String>,
    pub hit_result: String,
    pub damage_profile_id: Option<String>,
    pub damage_roll_request: Option<EffectRollRequest>,
    pub damage_packet_id: Option<String>,
    pub resulting_patches: Vec<StatePatch>,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
    pub world_tick: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct DamagePacket {
    pub damage_packet_id: String,
    #[serde(default)]
    pub effect_resolution_id: Option<String>,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub source_actor_id: Option<String>,
    pub target_actor_id: String,
    pub source_object_id: Option<String>,
    pub source_ability_id: Option<String>,
    pub contest_id: Option<String>,
    pub damage_expression: Option<String>,
    pub rolled_total: i32,
    pub damage_type: Option<String>,
    pub armor_interaction: serde_json::Value,
    pub final_hp_delta: i32,
    pub from_hp: Option<i32>,
    pub to_hp: Option<i32>,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
    pub world_tick: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RollKind { Attack, Damage, SkillCheck, OpposedCheck, SavingThrow, PercentileCheck, Sanity, Harm, Chaos, SpellEffect, Resistance, TableRoll, SecretNotice, ResourceLoss, EffectRoll, Unknown }
impl Default for RollKind { fn default() -> Self { Self::Unknown } }
impl RollKind { pub fn as_str(&self) -> &'static str { match self { Self::Attack => "attack", Self::Damage => "damage", Self::SkillCheck => "skill_check", Self::OpposedCheck => "opposed_check", Self::SavingThrow => "saving_throw", Self::PercentileCheck => "percentile_check", Self::Sanity => "sanity", Self::Harm => "harm", Self::Chaos => "chaos", Self::SpellEffect => "spell_effect", Self::Resistance => "resistance", Self::TableRoll => "table_roll", Self::SecretNotice => "secret_notice", Self::ResourceLoss => "resource_loss", Self::EffectRoll => "effect_roll", Self::Unknown => "unknown" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectTargetKind { Actor, Object, Scene, Clock, Relationship, Anomaly, Campaign, World }
impl Default for EffectTargetKind { fn default() -> Self { Self::Actor } }
impl EffectTargetKind { pub fn as_str(&self) -> &'static str { match self { Self::Actor => "actor", Self::Object => "object", Self::Scene => "scene", Self::Clock => "clock", Self::Relationship => "relationship", Self::Anomaly => "anomaly", Self::Campaign => "campaign", Self::World => "world" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParameterOperation { Add, Subtract, Set, AddCondition, RemoveCondition, AdvanceClock, MarkRevealed }
impl Default for ParameterOperation { fn default() -> Self { Self::Set } }
impl ParameterOperation { pub fn as_str(&self) -> &'static str { match self { Self::Add => "add", Self::Subtract => "subtract", Self::Set => "set", Self::AddCondition => "add_condition", Self::RemoveCondition => "remove_condition", Self::AdvanceClock => "advance_clock", Self::MarkRevealed => "mark_revealed" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct TargetRef {
    pub target_kind: EffectTargetKind,
    pub target_id: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RollPlan {
    pub roll_plan_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub roll_kind: RollKind,
    pub actor_id: Option<String>,
    pub target_refs: Vec<TargetRef>,
    pub dice_expression: String,
    pub parameters_used: serde_json::Value,
    pub target_model: serde_json::Value,
    pub authority: RollAuthority,
    pub visibility: RollVisibility,
    pub display_policy: serde_json::Value,
    pub expected_effects: Vec<String>,
    pub source_refs: Vec<SourceRef>,
    pub rule_binding_ids: Vec<String>,
    pub parameter_facet_ids: Vec<String>,
    pub world_tick: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ParameterImpact {
    pub impact_id: String,
    pub target_kind: EffectTargetKind,
    pub target_id: String,
    pub parameter_path: String,
    pub operation: ParameterOperation,
    pub value: serde_json::Value,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub validator_status: String,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct EffectResolutionPacket {
    pub effect_resolution_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub frame_id: Option<String>,
    pub source_actor_id: Option<String>,
    pub source_object_id: Option<String>,
    pub source_ability_id: Option<String>,
    pub source_event_id: Option<String>,
    pub target_refs: Vec<TargetRef>,
    pub roll_records: Vec<DiceRollRecord>,
    pub rule_binding_ids: Vec<String>,
    pub parameter_facet_ids: Vec<String>,
    pub impacts: Vec<ParameterImpact>,
    pub visibility: Visibility,
    pub source_refs: Vec<SourceRef>,
    pub provisional_reason: Option<String>,
    pub world_tick: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct CombatRoundTransition {
    pub action: CombatRoundAction,
    pub attack: Option<AttackResolutionContract>,
    pub damage: Option<DamagePacket>,
    pub actor_state: Option<ActorMechanicalState>,
    pub created_check_contracts: Vec<CheckContract>,
    pub created_effect_contracts: Vec<EffectContract>,
    pub proposed_patches: Vec<StatePatch>,
    pub narration_context: Option<String>,
}

// -----------------------------------------------------------------------------
// Contest / Opposition Kernel data contracts (v1.11)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContestKind { StaticCheck, OpposedCheck, Attack, Defense, SavingThrow, PercentileAbility, Resistance, RulesetProcedure, Provisional }
impl Default for ContestKind { fn default() -> Self { Self::Provisional } }
impl ContestKind { pub fn as_str(&self) -> &'static str { match self { Self::StaticCheck => "static_check", Self::OpposedCheck => "opposed_check", Self::Attack => "attack", Self::Defense => "defense", Self::SavingThrow => "saving_throw", Self::PercentileAbility => "percentile_ability", Self::Resistance => "resistance", Self::RulesetProcedure => "ruleset_procedure", Self::Provisional => "provisional" } } }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContestVerificationStatus { VerifiedExact, VerifiedPartial, ProvisionalNeedsBinding, RejectedNoSource, RejectedSchemaInvalid }
impl Default for ContestVerificationStatus { fn default() -> Self { Self::ProvisionalNeedsBinding } }
impl ContestVerificationStatus { pub fn as_str(&self) -> &'static str { match self { Self::VerifiedExact => "verified_exact", Self::VerifiedPartial => "verified_partial", Self::ProvisionalNeedsBinding => "provisional_needs_binding", Self::RejectedNoSource => "rejected_no_source", Self::RejectedSchemaInvalid => "rejected_schema_invalid" } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckResolutionModel {
    StaticTargetNumber { value: i32, label: String },
    OpposedRoll {
        attacker_expression: String,
        #[serde(default)]
        attacker_value: Option<i32>,
        defender_actor_id: Option<String>,
        defender_expression: String,
        #[serde(default)]
        defender_value: Option<i32>,
        defender_roll_visibility: RollVisibility,
    },
    AttackVsDefense { attack_expression: String, defender_actor_id: Option<String>, defense_label: String, defense_value: i32 },
    PercentileRollUnder { ability_label: String, ability_value: i32 },
    SavingThrow { dc: i32, save_label: String, defender_actor_ids: Vec<String> },
    RulesetProcedureLookup { procedure_label: String, unresolved_fields: Vec<String> },
    /// Count dice showing `target_face` in the rolled pool; success when the
    /// count >= `threshold`. Resolution reads the per-die array, not the sum.
    DicePoolCount { target_face: i32, threshold: i32, label: String },
    Provisional { reason: String, suggested_target: Option<i32> },
}
impl Default for CheckResolutionModel { fn default() -> Self { Self::Provisional { reason: "no contest model selected".into(), suggested_target: None } } }

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct OppositionProfile {
    pub opposition_id: String,
    pub session_id: String,
    pub check_id: String,
    pub defender_actor_id: Option<String>,
    pub defense_label: Option<String>,
    pub defense_value: Option<i32>,
    pub opposed_expression: Option<String>,
    pub source_refs: Vec<SourceRef>,
    pub confidence: RulingConfidence,
    pub provisional_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ContestProfile {
    pub contest_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub check_id: String,
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub contest_kind: ContestKind,
    pub attacker_actor_id: String,
    pub defender_actor_id: Option<String>,
    pub source_object_id: Option<String>,
    pub source_ability_id: Option<String>,
    pub resolution_model: CheckResolutionModel,
    pub roll_visibility: RollVisibility,
    pub target_summary: String,
    pub source_refs: Vec<SourceRef>,
    pub confidence: RulingConfidence,
    pub verification_status: ContestVerificationStatus,
    pub provisional_reason: Option<String>,
    pub world_tick: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ContestResolutionRecord {
    pub resolution_id: String,
    pub contest_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub check_id: String,
    pub roll_id: Option<String>,
    pub total: i64,
    pub target_value: Option<i64>,
    pub success: Option<bool>,
    pub degree: Option<String>,
    pub outcome_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod module_graph_compat_tests {
    use super::*;

    // 旧 bundle（无新字段）必须能反序列化，新字段取默认（骨架态）。
    #[test]
    fn old_scenario_node_json_deserializes_with_defaults() {
        let old = r#"{"node_id":"n1","title":"序幕","node_type":"chapter","summary":"s",
            "read_aloud":null,"gm_notes":null,"links":[],"assets":[],"data":null}"#;
        let n: ScenarioNode = serde_json::from_str(old).expect("旧 JSON 应可反序列化");
        assert_eq!(n.extraction_status, SceneExtractionStatus::SkeletonOnly);
        assert!(n.page_start.is_none());
        assert!(n.referenced_npc_ids.is_empty());
    }

    // 新字段 round-trip。
    #[test]
    fn scenario_node_roundtrips_new_fields() {
        let mut n = ScenarioNode::default();
        n.node_id = "loc1".into();
        n.extraction_status = SceneExtractionStatus::DeepExtracted;
        n.page_start = Some(17);
        n.referenced_npc_ids = vec!["npc_russell".into()];
        n.links = vec![ScenarioLink { to_node_id: "loc2".into(), reason: "可达".into(),
            clue_id: None, link_type: LinkType::Spatial, source_anchor: None }];
        let s = serde_json::to_string(&n).unwrap();
        let back: ScenarioNode = serde_json::from_str(&s).unwrap();
        assert_eq!(back.extraction_status, SceneExtractionStatus::DeepExtracted);
        assert_eq!(back.links[0].link_type, LinkType::Spatial);
    }

    // 旧 ScenarioLink（无 link_type）→ 默认 Sequential。
    #[test]
    fn old_link_defaults_to_sequential() {
        let l: ScenarioLink = serde_json::from_str(r#"{"to_node_id":"x","reason":"r","clue_id":null}"#).unwrap();
        assert_eq!(l.link_type, LinkType::Sequential);
    }

    // 旧 link（无 source_anchor）→ None；新 link round-trip。
    #[test]
    fn scenario_link_source_anchor_roundtrips_and_defaults() {
        // 旧 link(无 source_anchor)→ None
        let old: ScenarioLink = serde_json::from_str(r#"{"to_node_id":"b","reason":"r","clue_id":null}"#).unwrap();
        assert!(old.source_anchor.is_none(), "旧 link 无 source_anchor → None");
        // 新 link round-trip
        let l = ScenarioLink { to_node_id: "b".into(), reason: "门".into(), clue_id: None,
            link_type: LinkType::Trigger, source_anchor: Some("原文片段".into()) };
        let back: ScenarioLink = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back.source_anchor.as_deref(), Some("原文片段"));
    }

    // 锁定 snake_case wire 值（reader 与 parse_module 依赖这些字符串契约）。
    #[test]
    fn enum_wire_format_is_snake_case() {
        use serde_json::json;
        assert_eq!(serde_json::to_value(SceneExtractionStatus::SkeletonOnly).unwrap(), json!("skeleton_only"));
        assert_eq!(serde_json::to_value(SceneExtractionStatus::DeepExtracted).unwrap(), json!("deep_extracted"));
        assert_eq!(serde_json::to_value(ModuleContentClass::Bp2CustomRule).unwrap(), json!("bp2_custom_rule"));
        assert_eq!(serde_json::to_value(ModuleContentClass::Bp3Index).unwrap(), json!("bp3_index"));
        assert_eq!(serde_json::to_value(ModuleContentClass::Story).unwrap(), json!("story"));
        assert_eq!(serde_json::to_value(LinkType::Spatial).unwrap(), json!("spatial"));
        assert_eq!(serde_json::to_value(LinkType::Sequential).unwrap(), json!("sequential"));
    }
}

#[cfg(test)]
mod hp_resource_track_tests {
    use super::*;
    use serde_json::json;

    // kind=health 优先于 id 子串。
    #[test]
    fn health_kind_wins() {
        let tracks = vec![
            json!({"id": "hp", "kind": "track"}),
            json!({"id": "vitality", "kind": "health"}),
        ];
        assert_eq!(hp_resource_track_id(&tracks).as_deref(), Some("vitality"));
    }

    // 真实 CoC 形状: sanity + hit_points, 均 kind=track → 命中 hit_points。
    #[test]
    fn coc_shape_returns_hit_points() {
        let tracks = vec![
            json!({"id": "sanity", "kind": "track", "max": 99, "initial": 0, "owner_kind": "actor"}),
            json!({"id": "hit_points", "kind": "track", "max": 100, "initial": 0, "owner_kind": "actor"}),
        ];
        assert_eq!(hp_resource_track_id(&tracks).as_deref(), Some("hit_points"));
    }

    // 只有 sanity → 无 HP-like track → None (fail-closed)。
    #[test]
    fn no_hp_track_returns_none() {
        let tracks = vec![json!({"id": "sanity", "kind": "track"})];
        assert_eq!(hp_resource_track_id(&tracks), None);
    }

    // id == "hp" 命中。
    #[test]
    fn bare_hp_id_matches() {
        let tracks = vec![json!({"id": "hp", "kind": "track"})];
        assert_eq!(hp_resource_track_id(&tracks).as_deref(), Some("hp"));
    }
}

#[cfg(test)]
mod resource_helpers_tests {
    use super::*;
    use serde_json::json;

    fn coc_tracks() -> Vec<serde_json::Value> {
        vec![
            json!({"id":"sanity","kind":"track","max":99,"initial":0,"owner_kind":"actor"}),
            json!({"id":"hit_points","kind":"health","max":100,"initial":0,"owner_kind":"actor"}),
        ]
    }

    #[test]
    fn match_seed_uses_derived_value() {
        let seeds = match_seed(&json!({"sanity": 65}), &coc_tracks());
        assert_eq!(seeds.get("sanity"), Some(&(Some(65), Some(65))));
    }

    #[test]
    fn match_seed_falls_back_to_kernel_static() {
        let seeds = match_seed(&json!({}), &coc_tracks());
        assert_eq!(seeds.get("sanity"), Some(&(Some(0), Some(99))));
    }

    #[test]
    fn normalize_drops_sheet_field_defs_keeps_real_tracks() {
        let dirty = vec![
            json!({"field_id":"resources","field_type":"object","title":"Resources / Tracks"}),
            json!({"id":"hit_points","kind":"health","max":100}),
            json!({"name":"Stress"}),
        ];
        let clean = normalize_resource_tracks(&dirty);
        assert_eq!(clean.len(), 2);
        assert!(clean.iter().any(|t| t.get("id").and_then(|v| v.as_str()) == Some("hit_points")));
        assert!(clean.iter().any(|t| t.get("name").and_then(|v| v.as_str()) == Some("Stress")));
        assert!(!clean.iter().any(|t| t.get("field_id").is_some()));
    }

    #[test]
    fn resolve_track_id_maps_hp_alias_and_resource_path() {
        let k = RuleKernel { resource_tracks: coc_tracks(), ..Default::default() };
        assert_eq!(resolve_resource_track_id("hp.current", &k).as_deref(), Some("hit_points"));
        assert_eq!(resolve_resource_track_id("hp", &k).as_deref(), Some("hit_points"));
        assert_eq!(resolve_resource_track_id("resources.sanity.current", &k).as_deref(), Some("sanity"));
        assert_eq!(resolve_resource_track_id("resources.SANITY.current", &k).as_deref(), Some("sanity"));
        assert_eq!(resolve_resource_track_id("resources.unknown.current", &k), None);
    }

    #[test]
    fn armor_damage_math() {
        assert_eq!(apply_armor_damage(100, 15, None, ParameterOperation::Subtract), (85, 0));
        assert_eq!(apply_armor_damage(100, 15, Some(5), ParameterOperation::Subtract), (90, 5));
        assert_eq!(apply_armor_damage(100, 3, Some(5), ParameterOperation::Subtract), (100, 5));
        assert_eq!(apply_armor_damage(100, 15, Some(5), ParameterOperation::Add), (115, 0));
        assert_eq!(apply_armor_damage(100, 30, Some(5), ParameterOperation::Set), (30, 0));
        assert_eq!(apply_armor_damage(10, 50, None, ParameterOperation::Subtract), (0, 0));
    }

    #[test]
    fn wound_label_thresholds() {
        assert_eq!(wound_label(0, Some(20)), "defeated");
        assert_eq!(wound_label(-3, Some(20)), "defeated");
        assert_eq!(wound_label(8, Some(20)), "wounded");
        assert_eq!(wound_label(15, Some(20)), "unhurt");
        assert_eq!(wound_label(15, None), "unhurt");
    }
}

#[cfg(test)]
mod consumption_model_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opposed_roll_new_value_fields_default_none() {
        let old = json!({"kind":"opposed_roll","attacker_expression":"1d100",
            "defender_actor_id":"npc.opposition","defender_expression":"1d100",
            "defender_roll_visibility":"private_gm_roll"});
        let m: CheckResolutionModel = serde_json::from_value(old).unwrap();
        match m {
            CheckResolutionModel::OpposedRoll { attacker_value, defender_value, .. } => {
                assert_eq!(attacker_value, None);
                assert_eq!(defender_value, None);
            }
            _ => panic!("expected OpposedRoll"),
        }
    }

    #[test]
    fn contract_opponent_tested_parameter_defaults_none() {
        let v = serde_json::to_value(sample_min_contract()).unwrap();
        let mut obj = v.as_object().unwrap().clone();
        obj.remove("opponent_tested_parameter");
        let c: CheckContract = serde_json::from_value(serde_json::Value::Object(obj)).unwrap();
        assert!(c.opponent_tested_parameter.is_none());
    }

    fn sample_min_contract() -> CheckContract {
        serde_json::from_value(json!({
            "check_id":"c","session_id":"s","turn_id":"t","ruleset_id":"r","module_id":null,
            "initiator":{"actor_id":"pc.current","actor_kind":"player_character","display_name":null},
            "target_actor":null,"opposition":{"kind":"no_mechanical_opposition"},
            "action_summary":"","intent_kind":"","check_label":"","dice_expression":"1d100",
            "modifiers":[],"target":{"kind":"unknown_until_lookup"},"tested_parameter":null,
            "actor_snapshot_ids":[],"source_refs":[],"learned_packet_ids":[],
            "roll_visibility":"public_gm_roll","roll_authority":"system",
            "disclosure":{
                "show_roll_to_player":true,"show_formula_to_player":true,
                "show_dc_to_player":true,"show_success_failure_to_player":true,
                "reveal_after_scene":false,"reveal_after_session":false
            },
            "stakes":{"before_roll_public":"","success_public":"","failure_public":"",
                "critical_public":null,"fumble_public":null,"success_patches_allowed":[],
                "failure_patches_allowed":[],"irreversible":false},
            "confidence":"medium","ruling_status":"provisional","advice_refs":[],"expires_at_turn":null
        })).unwrap()
    }
}

#[cfg(test)]
mod derived_value_tier_tests {
    use super::*;
    use serde_json::json;

    // N1: tier field serde + helper methods

    #[test]
    fn tier_absent_deserializes_as_none_and_treated_as_exact() {
        // Old packs without "tier" must parse fine and default to exact.
        let v: DerivedValue = serde_json::from_value(json!({
            "field_id": "mechanic.attack", "formula": "1d20+5",
            "depends_on": [], "evaluator": "static_dc_contest"
        })).unwrap();
        assert_eq!(v.tier, None);
        assert!(v.is_exact_executable(), "absent tier => exact (backward compat)");
        assert!(!v.is_provisional_seed());
        assert!(!v.is_operational_abstract());
    }

    #[test]
    fn tier_provisional_seed_recognized() {
        let v: DerivedValue = serde_json::from_value(json!({
            "field_id": "mechanic.skill_check.total",
            "formula": "1d10 + STAT + Skill vs DV",
            "depends_on": ["stat","skill","dv"],
            "evaluator": "contest_profile",
            "tier": "provisional_seed"
        })).unwrap();
        assert_eq!(v.tier.as_deref(), Some("provisional_seed"));
        assert!(v.is_provisional_seed());
        assert!(!v.is_exact_executable());
        assert!(!v.is_operational_abstract());
    }

    #[test]
    fn tier_exact_executable_recognized() {
        let v: DerivedValue = serde_json::from_value(json!({
            "field_id": "hp_max", "formula": "floor(CON/2)+SIZ",
            "depends_on": ["con","siz"], "evaluator": "character_derived_value",
            "tier": "exact_executable"
        })).unwrap();
        assert_eq!(v.tier.as_deref(), Some("exact_executable"));
        assert!(v.is_exact_executable());
        assert!(!v.is_provisional_seed());
    }

    #[test]
    fn tier_operational_abstract_recognized() {
        let v: DerivedValue = serde_json::from_value(json!({
            "field_id": "mechanic.damage", "formula": "weapon damage table result",
            "depends_on": ["weapon"], "evaluator": "table_driven_effect_resolution",
            "tier": "operational_abstract"
        })).unwrap();
        assert!(v.is_operational_abstract());
        assert!(!v.is_exact_executable());
        assert!(!v.is_provisional_seed());
    }

    #[test]
    fn tier_roundtrips_through_serde() {
        let orig = DerivedValue {
            field_id: "mechanic.check".into(),
            formula: "1d10+STAT".into(),
            depends_on: vec!["stat".into()],
            evaluator: "contest_profile".into(),
            tier: Some("provisional_seed".into()),
            ..Default::default()
        };
        let json_val = serde_json::to_value(&orig).unwrap();
        assert_eq!(json_val["tier"], json!("provisional_seed"));
        let back: DerivedValue = serde_json::from_value(json_val).unwrap();
        assert_eq!(back.tier.as_deref(), Some("provisional_seed"));
    }
}

#[cfg(test)]
mod pp_lifecycle_tests {
    use super::{pp_lifecycle_rank, PP_STREAMING, PP_CRITICAL_DONE, PP_COMPLETE};

    #[test]
    fn lifecycle_ranks_are_strictly_monotonic() {
        // streaming < critical_done < complete —— 守卫据此判断「是否已达 critical」。
        assert!(pp_lifecycle_rank(PP_STREAMING) < pp_lifecycle_rank(PP_CRITICAL_DONE));
        assert!(pp_lifecycle_rank(PP_CRITICAL_DONE) < pp_lifecycle_rank(PP_COMPLETE));
    }

    #[test]
    fn unknown_lifecycle_ranks_lowest_fail_closed() {
        // 未知/旧值（如历史 'ready' 或脏数据）排最低 = 守卫视作「未达 critical」→
        // 触发短等而非误判已落账（fail-closed：宁可多等也不读陈旧）。
        assert!(pp_lifecycle_rank("ready") < pp_lifecycle_rank(PP_STREAMING));
        assert!(pp_lifecycle_rank("") < pp_lifecycle_rank(PP_STREAMING));
        assert!(pp_lifecycle_rank("garbage") < pp_lifecycle_rank(PP_STREAMING));
    }

    #[test]
    fn const_values_are_the_canonical_strings() {
        assert_eq!(PP_STREAMING, "streaming");
        assert_eq!(PP_CRITICAL_DONE, "critical_done");
        assert_eq!(PP_COMPLETE, "complete");
    }
}

#[cfg(test)]
mod referee_value_bands_tests {
    use super::*;

    #[test]
    fn referee_value_bands_roundtrip() {
        let k = RuleKernel {
            referee_value_bands: Some(RefereeValueBands {
                damage_family: "test_family".into(),
                damage_band: "1d4..2d8".into(),
                damage_plausible_range: (1, 10),
                difficulty_band: serde_json::json!({"common_band":"5..20"}),
                difficulty_plausible_range: (1, 40),
            }),
            ..Default::default()
        };
        let json = serde_json::to_value(&k).unwrap();
        assert!(json.pointer("/referee_value_bands/damage_family").is_some());
        let k2: RuleKernel = serde_json::from_value(json).unwrap();
        assert_eq!(k2.referee_value_bands.unwrap().damage_family, "test_family");
    }

    #[test]
    fn kernel_without_referee_value_bands_defaults_none() {
        let json = serde_json::json!({"kernel_id":"x","ruleset_id":"y","version":"1"});
        let k: RuleKernel = serde_json::from_value(json).unwrap();
        assert!(k.referee_value_bands.is_none(), "old kernels must not fail on missing field");
    }
}
