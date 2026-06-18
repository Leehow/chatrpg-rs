//! Runtime KnowledgeProjection（P0b）：把 DB 的 KnowledgeEdge 账本投影成运行时
//! 反剧透放行集。当前只暴露 player_party 已知事实（knows_true）—— spoiler_guard
//! 的 revealed 集统一从这里取，不再各处直连 list_revealed_facts。
//! NPC-as-holder 投影留待 P1（actor-id 统一后）。
use std::collections::HashSet;

use anyhow::Result;
use trpg_db::Db;

/// 玩家方知识投影：本会话 player_party 已确知为真（knows_true）的 fact_id 集。
pub struct KnowledgeProjection {
    pub revealed_fact_ids: HashSet<String>,
}

/// 从 KnowledgeEdge 账本拉取本会话 player_party 的 knows_true 事实集。
/// DB 抖动直接向上抛 Err，由调用方决定 fail-closed 策略（SceneNeedResolver
/// 取不到 → 空集 = 全部按未揭示裁剪，宁可不泄不赌 DB）。
pub async fn player_knowledge_projection(db: &Db, session_id: &str) -> Result<KnowledgeProjection> {
    let ids = db.list_player_known_fact_ids(session_id).await?;
    Ok(KnowledgeProjection {
        revealed_fact_ids: ids.into_iter().collect(),
    })
}
