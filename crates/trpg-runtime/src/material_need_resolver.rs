//! R2 MaterialNeedResolver: the materialization projection behind the Need bus.
//!
//! The resolver reproduces the original materialization-block logic
//! (WorldTimeService.current → world_tick →
//! MaterializationService::materialization_context_block). R2 收口后这是唯一的
//! materialization 投影路径——旧直连 `materialization_blocks_for_turn` 已删，
//! prepare_turn_context 只经此 resolver。
//!
//! Note: `materialization_context_block` is a pure DB read (four SELECT queries) with
//! zero search calls, so passing `search=None` to `MaterializationService::from_env`
//! is byte-equivalent to the legacy path that passes `self.search.clone()`.

use anyhow::Result;
use async_trait::async_trait;
use trpg_db::Db;
use trpg_material::MaterializationService;
use trpg_need::{MaterialNeed, Need, NeedKind, NeedOutcome, NeedResolver};
use trpg_time::WorldTimeService;

pub struct MaterialNeedResolver {
    db: Db,
}

impl MaterialNeedResolver {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn world_tick(&self, session_id: &str) -> i64 {
        WorldTimeService::new(self.db.clone())
            .current(session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default()
    }
}

#[async_trait]
impl NeedResolver for MaterialNeedResolver {
    fn kind(&self) -> NeedKind {
        NeedKind::Material
    }

    async fn resolve(&self, need: &Need) -> Result<NeedOutcome> {
        let Need::Material(MaterialNeed { scopes, .. }) = need else {
            // Defensive: bus routes by kind, so this should not happen.
            return Ok(NeedOutcome::default());
        };
        let world_tick = self.world_tick(&scopes.session_id).await;
        // search=None: materialization_context_block makes no search calls — pure DB reads.
        let block = MaterializationService::from_env(self.db.clone(), None)
            .materialization_context_block(&scopes.session_id, world_tick)
            .await?;
        Ok(NeedOutcome { blocks: vec![block], source_refs: vec![] })
    }
}
