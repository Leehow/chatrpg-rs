//! R2 EntityNeedResolver: wraps NPC on-demand synthesis (`ensure_npc_parameter`)
//! behind the Need bus.
//!
//! Unlike the scene/material/parameter resolvers (which produce `ContextBlock`s),
//! the Entity need is **side-effect dominant**: `ensure_npc_parameter` runs the
//! hybrid synthesis ladder (T1 source > T2 archetype > T3 persona-judge) and WRITES
//! the result onto the NPC's `sheet_json` (+ re-projects `mechanical_profile`). It
//! returns a value, not context blocks. So `resolve()` returns an EMPTY
//! `NeedOutcome` — the synthesis effect lives in the DB write, per the bus contract
//! comment in trpg-need (state mutations stay service-internal).
//!
//! Lazy LLM: the resolver holds only an `Arc<RuntimeEngine>`. The wrapped
//! `ensure_npc_parameter` builds its own LLM from env (`LlmConfig::from_env`) when
//! needed, so `EntityNeedResolver::new(engine)` stays cheap and the resolver never
//! requires an injected LLM. fail-closed everywhere: gate off / no LLM / no card →
//! `Ok(None)` → empty outcome; synthesis `Err` → warn + empty outcome (turn never
//! aborts).
//!
//! `TRPG_NEED_BUS_ENTITY=0` → the emit site (opposed prepass) skips the bus and
//! falls back to the original direct `ensure_npc_parameter` calls (kept until Task 7
//! removes them).

use std::sync::Arc;

use async_trait::async_trait;
use trpg_need::{Need, NeedKind, NeedOutcome, NeedResolver};

use crate::npc_synth::NpcPersona;
use crate::RuntimeEngine;

/// `entity_hint` wire format: `actor_id|bucket|param|check_context`. The emit site
/// (opposed prepass) encodes the four fields the wrapped `ensure_npc_parameter`
/// needs; the resolver decodes with `splitn(4, '|')` so a `'|'` inside the free-text
/// `check_context` is preserved in the 4th segment. Returns `None` (fail-closed) when
/// the hint is absent/empty or has fewer than 4 segments.
pub(crate) fn decode_entity_hint(hint: Option<&str>) -> Option<(String, String, String, String)> {
    let hint = hint?;
    if hint.is_empty() {
        return None;
    }
    let parts: Vec<&str> = hint.splitn(4, '|').collect();
    match parts.as_slice() {
        [aid, bkt, prm, ctx] if !aid.is_empty() => {
            Some((aid.to_string(), bkt.to_string(), prm.to_string(), ctx.to_string()))
        }
        _ => None,
    }
}

/// Encode the four synthesis fields into the `entity_hint` wire format. Kept next to
/// the decoder so encode/decode stay in lock-step (the emit site uses this).
pub fn encode_entity_hint(actor_id: &str, bucket: &str, param: &str, check_context: &str) -> String {
    format!("{actor_id}|{bucket}|{param}|{check_context}")
}

/// EntityNeedResolver: thin adapter over `RuntimeEngine::ensure_npc_parameter`.
/// Claims `NeedKind::Entity`. Side-effect dominant (writes the NPC card); returns
/// empty blocks. Re-implements NO synthesis logic — the engine owns the ladder.
pub struct EntityNeedResolver {
    engine: Arc<RuntimeEngine>,
}

impl EntityNeedResolver {
    pub fn new(engine: Arc<RuntimeEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl NeedResolver for EntityNeedResolver {
    fn kind(&self) -> NeedKind {
        NeedKind::Entity
    }

    async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome> {
        // Bus routes by kind, so Entity is the only expected branch; treat anything
        // else defensively (fail-closed: empty outcome, never abort the turn).
        let Need::Entity(e) = need else {
            tracing::warn!(got = ?need.kind(), "EntityNeedResolver received non-entity need; empty outcome");
            return Ok(NeedOutcome::default());
        };
        let entity = decode_entity_hint(e.entity_hint.as_deref());
        let Some((actor_id, bucket, param, check_ctx)) = entity else {
            tracing::warn!(target: "entity_need_resolver",
                hint = ?e.entity_hint,
                "EntityNeed hint missing/malformed (want actor_id|bucket|param|ctx) — skipping synthesis");
            return Ok(NeedOutcome::default());
        };
        // name/prose cannot be reconstructed from the hint; synthesis routing (T1/T2)
        // does not depend on them, and T3 persona-judge with empty prose still
        // fail-closes to a default LLM judgement. The emit site already wrote the card
        // shell; here we only ensure the parameter is materialized.
        let persona = NpcPersona {
            actor_id: actor_id.clone(),
            name: actor_id.clone(),
            prose: String::new(),
        };
        // Side effect: write the card. Mirrors the direct `ensure_npc_parameter` call
        // in opposed_prepass. fail-closed: Ok(None) gate-off/no-LLM/no-card → empty;
        // Err → warn + empty.
        match self
            .engine
            .ensure_npc_parameter(&e.scopes.session_id, &e.scopes.ruleset_id, &persona, &bucket, &param, &check_ctx)
            .await
        {
            Ok(Some(_)) => {
                tracing::info!(target: "entity_need_resolver",
                    actor_id = %actor_id, bucket = %bucket, param = %param,
                    "NPC param synthesized via NeedBus");
            }
            Ok(None) => {
                tracing::info!(target: "entity_need_resolver",
                    actor_id = %actor_id,
                    "synthesis unavailable (gate off / no LLM / no card) — ok");
            }
            Err(err) => {
                tracing::warn!(target: "entity_need_resolver",
                    error = %err, actor_id = %actor_id,
                    "synthesis failed (fail-closed, turn continues)");
            }
        }
        // blocks always empty — side-effect dominant; context assembly never depends
        // on EntityNeed producing blocks.
        Ok(NeedOutcome::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use trpg_need::{EntityNeed, Need, NeedBus, NeedKind, NeedOutcome, NeedResolver, NeedScopes};

    fn scopes() -> NeedScopes {
        NeedScopes {
            ruleset_id: "call_of_cthulhu_7e".into(),
            module_id: Some("blood_highway".into()),
            session_id: "sess_test".into(),
            turn_id: "turn_001".into(),
            scene_id: Some("sc01".into()),
        }
    }

    // ---- hint codec (pure) ----

    #[test]
    fn hint_roundtrips_all_four_fields() {
        let h = encode_entity_hint("npc.ras", "skills", "perception", "opposed combat prepass");
        let (a, b, p, c) = decode_entity_hint(Some(&h)).expect("decode");
        assert_eq!(a, "npc.ras");
        assert_eq!(b, "skills");
        assert_eq!(p, "perception");
        assert_eq!(c, "opposed combat prepass");
    }

    #[test]
    fn hint_pipe_in_ctx_survives_splitn4() {
        // check_context itself contains '|' → 4th segment keeps the whole tail.
        let (_, _, _, c) = decode_entity_hint(Some("npc.x|stats|defense|context with | pipe inside")).unwrap();
        assert_eq!(c, "context with | pipe inside");
    }

    #[test]
    fn hint_fail_closed_on_missing_or_short() {
        assert!(decode_entity_hint(None).is_none(), "absent hint → None");
        assert!(decode_entity_hint(Some("")).is_none(), "empty hint → None");
        assert!(decode_entity_hint(Some("npc.x|stats|defense")).is_none(), "3 segments → None");
        assert!(decode_entity_hint(Some("|stats|defense|ctx")).is_none(), "empty actor_id → None");
    }

    // ---- bus routing + side-effect semantics (stub resolver, no DB/LLM) ----
    // Mirrors the real EntityNeedResolver's contract: claims Entity, side-effect
    // counted, empty blocks. (The real resolver needs an Arc<RuntimeEngine>+DB+LLM,
    // exercised by the SKIP-gated live path and by opposed_prepass A/B e2e.)

    struct CountingEntityResolver {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl NeedResolver for CountingEntityResolver {
        fn kind(&self) -> NeedKind {
            NeedKind::Entity
        }
        async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match need {
                Need::Entity(_) => Ok(NeedOutcome::default()),
                _ => anyhow::bail!("wrong kind"),
            }
        }
    }

    #[tokio::test]
    async fn entity_resolver_returns_empty_blocks_side_effect_is_counted() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver = CountingEntityResolver { calls: calls.clone() };
        assert_eq!(resolver.kind(), NeedKind::Entity, "must claim Entity kind");
        let mut bus = NeedBus::new();
        bus.register(Box::new(resolver));
        bus.emit(Need::Entity(EntityNeed {
            scopes: scopes(),
            entity_hint: Some(encode_entity_hint("npc.ras", "skills", "perception", "ctx")),
        }));
        let outcomes = bus.resolve_all().await;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].blocks.is_empty(), "EntityNeedResolver must return empty blocks");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "resolve() must be called once per emit");
    }

    #[tokio::test]
    async fn entity_resolver_fail_closed_on_error_does_not_panic_bus() {
        struct AlwaysFailResolver;
        #[async_trait]
        impl NeedResolver for AlwaysFailResolver {
            fn kind(&self) -> NeedKind {
                NeedKind::Entity
            }
            async fn resolve(&self, _: &Need) -> anyhow::Result<NeedOutcome> {
                anyhow::bail!("synthesis LLM unavailable")
            }
        }
        let mut bus = NeedBus::new();
        bus.register(Box::new(AlwaysFailResolver));
        bus.emit(Need::Entity(EntityNeed {
            scopes: NeedScopes {
                ruleset_id: "r".into(),
                module_id: None,
                session_id: "s".into(),
                turn_id: "t".into(),
                scene_id: None,
            },
            entity_hint: None,
        }));
        // fail-closed: bus does not panic; returns a (single) empty outcome.
        let outcomes = bus.resolve_all().await;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].blocks.is_empty(), "failed resolver must yield empty outcome");
    }
}
