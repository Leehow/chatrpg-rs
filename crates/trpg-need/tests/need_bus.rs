use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use trpg_model::{
    BlockContent, BlockKind, CacheZone, ContextBlock, Scope, SourceRef, Stability, Visibility,
};
use trpg_need::{
    EntityNeed, MaterialNeed, Need, NeedBus, NeedKind, NeedOutcome, NeedResolver, NeedScopes,
    ParameterNeed, RuleNeed, RuleNeedKind, SceneNeed,
};

fn scopes() -> NeedScopes {
    NeedScopes {
        ruleset_id: "rs".into(),
        module_id: None,
        session_id: "sess".into(),
        turn_id: "t1".into(),
        scene_id: None,
    }
}

fn rule_need() -> Need {
    Need::Rule(RuleNeed {
        ruleset_id: "rs".into(),
        need_kind: RuleNeedKind::GeneralRuleQuery,
        ..Default::default()
    })
}

fn sample_block(id: &str) -> ContextBlock {
    ContextBlock::new(
        id.to_string(),
        BlockKind::LookupResult,
        "t".to_string(),
        BlockContent::Text("body".into()),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope::ruleset("rs".to_string()),
        50,
    )
}

/// Records how many times resolve() ran and emits one block tagged with its kind.
struct CountingResolver {
    kind: NeedKind,
    calls: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl NeedResolver for CountingResolver {
    fn kind(&self) -> NeedKind {
        self.kind
    }
    async fn resolve(&self, _need: &Need) -> anyhow::Result<NeedOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(NeedOutcome {
            blocks: vec![sample_block("counting.block")],
            source_refs: vec![SourceRef::default()],
        })
    }
}

/// Always errors — bus must fail-closed (warn + empty outcome), never panic/abort the batch.
struct ErroringResolver;
#[async_trait::async_trait]
impl NeedResolver for ErroringResolver {
    fn kind(&self) -> NeedKind {
        NeedKind::Material
    }
    async fn resolve(&self, _need: &Need) -> anyhow::Result<NeedOutcome> {
        anyhow::bail!("boom")
    }
}

#[test]
fn need_kind_covers_five_variants() {
    assert_eq!(rule_need().kind(), NeedKind::Rule);
    assert_eq!(
        Need::Material(MaterialNeed {
            scopes: scopes(),
            user_input: None
        })
        .kind(),
        NeedKind::Material
    );
    assert_eq!(
        Need::Scene(SceneNeed {
            scopes: scopes(),
            project_module_ids: vec![]
        })
        .kind(),
        NeedKind::Scene
    );
    assert_eq!(
        Need::Entity(EntityNeed {
            scopes: scopes(),
            entity_hint: None
        })
        .kind(),
        NeedKind::Entity
    );
    assert_eq!(
        Need::Parameter(ParameterNeed {
            scopes: scopes(),
            actor_id: None,
            current_input: None
        })
        .kind(),
        NeedKind::Parameter
    );
}

#[tokio::test]
async fn resolve_all_routes_by_kind() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut bus = NeedBus::new();
    bus.register(Box::new(CountingResolver {
        kind: NeedKind::Rule,
        calls: calls.clone(),
    }));
    bus.emit(rule_need());
    let outcomes = bus.resolve_all().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "matching resolver invoked once"
    );
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].blocks.len(), 1);
    assert_eq!(outcomes[0].source_refs.len(), 1);
}

#[tokio::test]
async fn unclaimed_kind_is_skipped_not_errored() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut bus = NeedBus::new();
    // Only a Rule resolver registered; emit a Scene need (unclaimed).
    bus.register(Box::new(CountingResolver {
        kind: NeedKind::Rule,
        calls: calls.clone(),
    }));
    bus.emit(Need::Scene(SceneNeed {
        scopes: scopes(),
        project_module_ids: vec![],
    }));
    let outcomes = bus.resolve_all().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "rule resolver not called for scene need"
    );
    assert!(
        outcomes.is_empty(),
        "unclaimed need yields no outcome, no panic"
    );
}

#[tokio::test]
async fn resolver_error_is_fail_closed_empty_outcome() {
    let mut bus = NeedBus::new();
    bus.register(Box::new(ErroringResolver));
    bus.emit(Need::Material(MaterialNeed {
        scopes: scopes(),
        user_input: None,
    }));
    let outcomes = bus.resolve_all().await;
    // Error → warn + empty NeedOutcome appended (turn not aborted), so exactly one empty outcome.
    assert_eq!(
        outcomes.len(),
        1,
        "erroring resolver still yields one (empty) outcome"
    );
    assert!(outcomes[0].blocks.is_empty());
    assert!(outcomes[0].source_refs.is_empty());
}

#[tokio::test]
async fn pending_drained_after_resolve_all() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut bus = NeedBus::new();
    bus.register(Box::new(CountingResolver {
        kind: NeedKind::Rule,
        calls: calls.clone(),
    }));
    bus.emit(rule_need());
    let _ = bus.resolve_all().await;
    // Second drain with no new emits → nothing runs.
    let outcomes = bus.resolve_all().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "no re-resolution of drained needs"
    );
    assert!(outcomes.is_empty());
}

/// Compile-time proof the re-export is the SAME type as trpg_model's (single definition).
#[test]
fn rule_need_reexport_is_model_type() {
    let m: trpg_model::RuleNeed = RuleNeed {
        ruleset_id: "rs".into(),
        ..Default::default()
    };
    let n: trpg_need::RuleNeed = m; // moves between aliases → identical type
    assert_eq!(n.need_kind.as_str(), "general_rule_query");
}
