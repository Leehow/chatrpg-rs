//! R2 unified Need bus: typed turn-scoped data-acquisition requests + a kind-routed
//! resolver registry. Low-level crate — depends only on trpg-model. Resolvers (which
//! wrap runtime services) live in trpg-runtime, not here.
use trpg_model::{ContextBlock, SourceRef};

// RuleNeed/RuleNeedKind are canonically defined in trpg-model and consumed by
// trpg-rule-agent via `use trpg_model::*`. We re-export the SAME type (single
// definition, zero duplication) so callers can refer to `trpg_need::RuleNeed`.
pub use trpg_model::{RuleNeed, RuleNeedKind};

/// Discriminant used to route a Need to the resolver that claims that kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedKind {
    Rule,
    Material,
    Scene,
    Entity,
    Parameter,
}

/// Turn-scoped addressing shared by every Need payload. No per-ruleset fields:
/// all ids are opaque strings filled by the turn pipeline.
#[derive(Debug, Clone)]
pub struct NeedScopes {
    pub ruleset_id: String,
    pub module_id: Option<String>,
    pub session_id: String,
    pub turn_id: String,
    pub scene_id: Option<String>,
}

/// A typed data-acquisition request emitted during turn-context assembly.
#[derive(Debug, Clone)]
pub enum Need {
    Rule(RuleNeed),
    Material(MaterialNeed),
    Scene(SceneNeed),
    Entity(EntityNeed),
    Parameter(ParameterNeed),
}

impl Need {
    pub fn kind(&self) -> NeedKind {
        match self {
            Need::Rule(_) => NeedKind::Rule,
            Need::Material(_) => NeedKind::Material,
            Need::Scene(_) => NeedKind::Scene,
            Need::Entity(_) => NeedKind::Entity,
            Need::Parameter(_) => NeedKind::Parameter,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MaterialNeed {
    pub scopes: NeedScopes,
    pub user_input: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SceneNeed {
    pub scopes: NeedScopes,
    /// project.modules 中所有 module_id 列表，用于 resolver 做成员校验（防跨 project 泄漏）。
    /// 由 prepare_turn_context 在构造 Need 时填入（project 已在调用方加载）。
    pub project_module_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EntityNeed {
    pub scopes: NeedScopes,
    pub entity_hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParameterNeed {
    pub scopes: NeedScopes,
    pub actor_id: Option<String>,
    pub current_input: Option<String>,
}

/// What a resolver returns: context blocks to merge + source refs for grounding.
/// State mutations (e.g. NPC synth writing sheet_json) stay as service-internal
/// side effects inside the resolver's wrapped service — they do NOT travel here.
#[derive(Debug, Default)]
pub struct NeedOutcome {
    pub blocks: Vec<ContextBlock>,
    pub source_refs: Vec<SourceRef>,
}

/// One resolver claims exactly one NeedKind and turns a Need into a NeedOutcome.
#[async_trait::async_trait]
pub trait NeedResolver: Send + Sync {
    fn kind(&self) -> NeedKind;
    async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome>;
}

/// Kind-routed registry. Collects emitted Needs, then resolves the whole batch.
pub struct NeedBus {
    resolvers: Vec<Box<dyn NeedResolver>>,
    pending: Vec<Need>,
}

impl Default for NeedBus {
    fn default() -> Self {
        Self::new()
    }
}

impl NeedBus {
    pub fn new() -> Self {
        Self {
            resolvers: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub fn register(&mut self, r: Box<dyn NeedResolver>) {
        self.resolvers.push(r);
    }

    pub fn emit(&mut self, need: Need) {
        self.pending.push(need);
    }

    /// Drain pending needs and resolve each via the resolver whose kind matches.
    /// fail-closed: an unclaimed kind warns and is skipped (no outcome); a resolver
    /// Err warns and contributes an empty NeedOutcome — the turn is never aborted.
    pub async fn resolve_all(&mut self) -> Vec<NeedOutcome> {
        let needs = std::mem::take(&mut self.pending);
        let mut outcomes = Vec::with_capacity(needs.len());
        for need in &needs {
            let want = need.kind();
            let Some(resolver) = self.resolvers.iter().find(|r| r.kind() == want) else {
                tracing::warn!(
                    ?want,
                    "no NeedResolver claims this need kind; skipping (fail-closed)"
                );
                continue;
            };
            match resolver.resolve(need).await {
                Ok(outcome) => outcomes.push(outcome),
                Err(err) => {
                    tracing::warn!(error = %err, ?want, "NeedResolver failed; empty outcome (fail-closed)");
                    outcomes.push(NeedOutcome::default());
                }
            }
        }
        outcomes
    }
}
