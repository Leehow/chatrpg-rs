//! R2 Need-bus resolvers that live in trpg-runtime (they wrap runtime services).
//!
//! Task 2 lands the first one: `RuleNeedResolver`, a thin adapter over
//! `RuleStewardAgent::assist`. It does NOT re-implement any retrieval logic — the
//! steward already does learned-packet matching + Tantivy search + locator/rg
//! fallback + source-ref grounding. The resolver simply moves the assist's two
//! context-bearing fields (`context_blocks`, `source_refs`) into a `NeedOutcome`.

use std::path::PathBuf;

use trpg_db::Db;
use trpg_model::RuleAssist;
use trpg_need::{Need, NeedKind, NeedOutcome, NeedResolver};
use trpg_rule_agent::RuleStewardAgent;
use trpg_search::SearchService;

/// Pure mapping: a `RuleAssist`'s context_blocks/source_refs → a `NeedOutcome`.
/// No IO, no side effects — keeps the field-for-field contract unit-testable and
/// makes the resolver a trivial wrapper around `assist`.
pub(crate) fn rule_assist_to_outcome(assist: RuleAssist) -> NeedOutcome {
    NeedOutcome {
        blocks: assist.context_blocks,
        source_refs: assist.source_refs,
    }
}

/// Where the steward looks for parsed/markdown source artifacts (rg fallback).
/// Mirrors the CLI's `default_data_dir()` env contract: `TRPG_DATA_DIR`, else `data`.
pub(crate) fn runtime_steward_data_dir() -> PathBuf {
    std::env::var("TRPG_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"))
}

/// Claims `NeedKind::Rule`. Resolves a `Need::Rule(RuleNeed)` by delegating to the
/// rule steward's deterministic, source-backed `assist` (no LLM in that path).
pub(crate) struct RuleNeedResolver {
    steward: RuleStewardAgent,
}

impl RuleNeedResolver {
    pub(crate) fn new(db: Db, search: SearchService, data_dir: PathBuf) -> Self {
        Self {
            steward: RuleStewardAgent::new(db, search, data_dir),
        }
    }
}

#[async_trait::async_trait]
impl NeedResolver for RuleNeedResolver {
    fn kind(&self) -> NeedKind {
        NeedKind::Rule
    }

    async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome> {
        // The bus routes by kind, so this is the only branch we expect; treat any
        // other variant defensively (fail-closed: empty outcome, never abort the turn).
        let Need::Rule(rule_need) = need else {
            tracing::warn!(got = ?need.kind(), "RuleNeedResolver received non-rule need; returning empty outcome");
            return Ok(NeedOutcome::default());
        };
        // `assist` takes RuleNeed by value; the bus hands us a `&Need`, so clone the
        // routed payload to give the steward ownership.
        let assist = self.steward.assist(rule_need.clone()).await?;
        Ok(rule_assist_to_outcome(assist))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{
        BlockContent, BlockKind, CacheZone, ContextBlock, Scope, SourceRef, Stability, Visibility,
    };

    fn sample_block(id: &str) -> ContextBlock {
        ContextBlock::new(
            id.to_string(),
            BlockKind::LookupResult,
            "hit",
            BlockContent::Text("body".into()),
            Visibility::GmOnly,
            Stability::TurnDynamic,
            CacheZone::DynamicTail,
            Scope::ruleset("coc"),
            78,
        )
    }

    #[test]
    fn assist_blocks_and_refs_map_verbatim_into_outcome() {
        let assist = RuleAssist {
            context_blocks: vec![sample_block("a"), sample_block("b")],
            source_refs: vec![SourceRef {
                source_id: "rulebook".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let outcome = rule_assist_to_outcome(assist);
        assert_eq!(
            outcome.blocks.len(),
            2,
            "two assist blocks must survive into outcome"
        );
        assert_eq!(outcome.blocks[0].block_id, "a");
        assert_eq!(outcome.blocks[1].block_id, "b");
        assert!(
            !outcome.source_refs.is_empty(),
            "source_refs grounding must carry over (fail-closed grounding assertion)"
        );
        assert_eq!(outcome.source_refs[0].source_id, "rulebook");
    }

    #[test]
    fn empty_assist_yields_empty_outcome_not_panic() {
        let outcome = rule_assist_to_outcome(RuleAssist::default());
        assert!(outcome.blocks.is_empty() && outcome.source_refs.is_empty());
    }

    #[test]
    fn steward_data_dir_honors_env_then_falls_back() {
        // Default (env unset) → "data"; explicit env → that path.
        std::env::remove_var("TRPG_DATA_DIR");
        assert_eq!(runtime_steward_data_dir(), PathBuf::from("data"));
        std::env::set_var("TRPG_DATA_DIR", "/tmp/coc-data");
        assert_eq!(runtime_steward_data_dir(), PathBuf::from("/tmp/coc-data"));
        std::env::remove_var("TRPG_DATA_DIR");
    }
}
