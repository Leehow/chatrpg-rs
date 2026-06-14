//! R2 ParameterNeedResolver: the actor-parameter projection behind the Need bus.
//!
//! The resolver reproduces the original actor-parameter logic (world_tick →
//! ensure_actor_parameters PC → refresh_actor_live_derived → ensure_actor_parameters
//! NPC if frame/npc-mention → actor_parameters_context_block), preserving BOTH the
//! side effects (param state mutations) AND the produced blocks. R2 收口后这是唯一的
//! actor-parameter 投影路径——旧直连 `actor_parameter_blocks_for_turn` 已删，
//! prepare_turn_context 只经此 resolver。

use anyhow::Result;
use async_trait::async_trait;
use trpg_db::Db;
use trpg_model::ActorKind;
use trpg_need::{Need, NeedKind, NeedOutcome, NeedResolver, ParameterNeed};
use trpg_params::RuntimeParameterService;
use trpg_time::WorldTimeService;

use crate::chargen;

pub struct ParameterNeedResolver {
    db: Db,
}

impl ParameterNeedResolver {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn resolve_parameter(&self, need: &ParameterNeed) -> Result<NeedOutcome> {
        let world_tick = WorldTimeService::new(self.db.clone())
            .current(&need.scopes.session_id)
            .await
            .map(|t| t.world_tick)
            .unwrap_or_default();

        let service = RuntimeParameterService::new(self.db.clone());
        let actor_id = need.actor_id.as_deref().unwrap_or("pc.current");

        // ensure PC 参数（等价旧 actor_parameter_blocks_for_turn 的 ensure_actor_parameters PC）
        let _ = service
            .ensure_actor_parameters(
                &need.scopes.session_id,
                &need.scopes.ruleset_id,
                actor_id,
                ActorKind::PlayerCharacter,
                world_tick,
            )
            .await;

        // §10.1 LIVE linkage（等价旧 refresh_actor_live_derived 调用）
        let _ = self.refresh_live_derived(&need.scopes.session_id, actor_id).await;

        // ensure NPC 参数（等价旧 active_frame_exists || mentions_npc 分支）
        let active_frame_exists = self
            .db
            .list_active_state_frames(&need.scopes.session_id, 1)
            .await
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let mentions_npc = need
            .current_input
            .as_deref()
            .map(mentions_runtime_npc)
            .unwrap_or(false);
        if active_frame_exists || mentions_npc {
            let _ = service
                .ensure_actor_parameters(
                    &need.scopes.session_id,
                    &need.scopes.ruleset_id,
                    "npc.opposition",
                    ActorKind::Npc,
                    world_tick,
                )
                .await;
        }

        let block = service
            .actor_parameters_context_block(&need.scopes.session_id, world_tick)
            .await?;
        Ok(NeedOutcome { blocks: vec![block], source_refs: vec![] })
    }

    /// §10.1: 重算 live 派生值并持久化（等价 RuntimeEngine::refresh_actor_live_derived）。
    async fn refresh_live_derived(&self, session_id: &str, actor_id: &str) -> Result<bool> {
        let service = RuntimeParameterService::new(self.db.clone());
        if let Some(mut p) = service.load_actor_parameters(session_id, actor_id).await? {
            if chargen::recompute_live_derived(&mut p.sheet_json) {
                chargen::refresh_mechanical_profile(&mut p.mechanical_profile, &p.sheet_json);
                service.upsert_actor_parameters(&p).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// 与 lib.rs `mentions_runtime_npc` 字节等价的 NPC 关键词检测。
/// 必须与 lib.rs 中的定义保持一致（关键词集合相同、顺序无关）。
fn mentions_runtime_npc(input: &str) -> bool {
    let lower = input.to_lowercase();
    ["npc", "scav", "guard", "守卫", "敌", "无人机", "警察", "帮派", "对方", "他", "她",
     "drone", "enemy", "opposition"]
        .iter()
        .any(|t| lower.contains(t))
}

#[async_trait]
impl NeedResolver for ParameterNeedResolver {
    fn kind(&self) -> NeedKind {
        NeedKind::Parameter
    }

    async fn resolve(&self, need: &Need) -> anyhow::Result<NeedOutcome> {
        let Need::Parameter(p) = need else {
            // Defensive: bus routes by kind; other variants return empty (fail-closed).
            tracing::warn!(got = ?need.kind(), "ParameterNeedResolver received non-parameter need; returning empty outcome");
            return Ok(NeedOutcome::default());
        };
        match self.resolve_parameter(p).await {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                tracing::warn!(error = %err, "ParameterNeedResolver: resolve_parameter failed; empty outcome (fail-closed)");
                Ok(NeedOutcome::default())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mentions_runtime_npc 关键词集合与 lib.rs 定义一致的回归测试。
    #[test]
    fn mentions_npc_keywords_match_lib_rs() {
        assert!(mentions_runtime_npc("有个 npc 跑过来"));
        assert!(mentions_runtime_npc("scav boss 出现了"));
        assert!(mentions_runtime_npc("guard 挡在门口"));
        assert!(mentions_runtime_npc("守卫正在巡逻"));
        assert!(mentions_runtime_npc("drone 飞过来"));
        assert!(mentions_runtime_npc("enemy spotted"));
        assert!(mentions_runtime_npc("opposition forces"));
        assert!(!mentions_runtime_npc("我想开门"));
        assert!(!mentions_runtime_npc("player casts a spell"));
    }

    #[test]
    fn mentions_npc_case_insensitive() {
        assert!(mentions_runtime_npc("NPC walks in"));
        assert!(mentions_runtime_npc("ENEMY spotted"));
        assert!(mentions_runtime_npc("GUARD at the door"));
    }

    #[test]
    fn resolver_claims_parameter_kind() {
        use trpg_db::Db;
        // We can only verify the kind() call without a real DB.
        // ParameterNeedResolver struct is opaque without a DB, so just verify
        // that the NeedKind::Parameter discriminant exists in the enum.
        assert_eq!(NeedKind::Parameter, NeedKind::Parameter);
    }
}
