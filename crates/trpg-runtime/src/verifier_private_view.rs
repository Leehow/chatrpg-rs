//! P3.5 — typed AfterStream verifier input (`VerifierPrivateView`).
//!
//! 把「流后泄漏校验」所需的玩家叙事投影 + 活动 NPC 言谈投影**一次性、确定性**装配成
//! 一个 owned 私有视图，供 trpg-gm 的 AfterLlmStream 校验路径单点消费（替换原来散落在
//! `run_after_llm_stream_hook` / `npc_consistency_after_stream` 里的多次 DB 读）。
//!
//! ## 不变量（INVARIANT — 绝不渲染进 prompt）
//! `VerifierPrivateView` 是**纯私有 verifier 输入**：它聚合的玩家未知/NPC 自有知识只能流向
//! 私有泄漏/一致性校验器，**绝不**进入任何模型 prompt。为此本类型刻意：
//! - 不派生 `Serialize`；
//! - 不提供任何 `to_block` / `render_*` / `to_context_block` / `into_prompt` 方法。
//! 见下方 `prompt_safety_invariant` 模块级守卫测试（编译期 + grep 级双保险）。
//!
//! ## crate 洁净
//! `secret_terms` 是 trpg-gm 概念（从模组图谱采集的 [`SecretTerm`]），而 trpg-runtime **不**
//! 依赖 trpg-gm。故本视图**只**承载 runtime-clean 的两段投影；secret_terms 仍在 trpg-gm 的
//! verify 路径里采集、并在那里与本视图组合。
//!
//! ## fail-closed（字段独立）
//! [`build_verifier_private_view`] 的每个字段独立 fail-closed：player_known 取数失败 → 空投影
//! （绝不中止整次 build，NPC 视图照建）；某个 NPC 的 profile/mind 载入失败 → 只跳过该 NPC，
//! 其余 NPC 视图保留。任何失败只 warn，绝不 panic、绝不阻断 build。
use trpg_db::Db;
use trpg_model::NpcRelationshipTarget;

use crate::knowledge_projection::{
    project_for_npc_speech, project_for_player_narration, NpcSpeechProjection,
    PlayerNarrationProjection,
};

/// 流后泄漏校验的私有输入视图（**绝不进 prompt**，见模块文档不变量）。
///
/// 只承载 runtime-clean 的两段投影：
/// - `player_known`：玩家叙事投影（玩家已知/已揭示 fact 放行集）；
/// - `npc_speech_views`：活动 NPC 的言谈投影（每个只含该 NPC 自有 mind/plan）。
///
/// secret_terms（trpg-gm 概念）**不**在此结构——它在 trpg-gm verify 路径里单独采集后组合。
///
/// 刻意不派生 `Serialize`、不提供任何渲染/转 block 方法（fail-closed 防泄漏不变量）。
#[derive(Debug, Clone)]
pub struct VerifierPrivateView {
    /// 玩家叙事投影（player-safe：只含玩家已知 fact）。
    pub player_known: PlayerNarrationProjection,
    /// 活动 NPC 的言谈投影（display_name + 该 NPC 自有言谈投影）。
    pub npc_speech_views: Vec<NpcSpeechProjection>,
}

impl VerifierPrivateView {
    /// 纯构造（无 IO，供单测与缓存）。
    pub fn new(
        player_known: PlayerNarrationProjection,
        npc_speech_views: Vec<NpcSpeechProjection>,
    ) -> Self {
        Self {
            player_known,
            npc_speech_views,
        }
    }
}

/// 一次性、确定性装配 [`VerifierPrivateView`]（async，**字段独立 fail-closed**）。
///
/// - `player_known`：经 [`project_for_player_narration`] 拉取；Err → 空投影（不中止 build）。
/// - `npc_speech_views`：对每个 `active_npc_ids`，载其 durable profile 后经
///   [`project_for_npc_speech`]（relationship_targets = `[PlayerParty]`，与 turn_loop 既有约定
///   一致）投出言谈投影，secret 门按 player_known 派生的玩家已知集。**每 NPC 一次
///   `load_npc_mind_view`**（不另载行动投影——无消费者）。单个 NPC 的 profile 缺失/读失败/投影
///   失败 → 只跳过该 NPC，其余保留。
///
/// 任何失败只 warn，绝不 panic、绝不阻断 build（fail-closed：宁可少校验，不赌 DB / 不拦流）。
pub async fn build_verifier_private_view(
    db: &Db,
    session_id: &str,
    active_npc_ids: &[String],
) -> VerifierPrivateView {
    // 字段独立 fail-closed：player_known 取数失败 → 空投影，NPC 视图仍照建。
    let player_known = match project_for_player_narration(db, session_id).await {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(error = %err, session_id, "verifier_private_view: player narration projection failed; using empty projection (fail-closed)");
            PlayerNarrationProjection::default()
        }
    };
    let player_known_fact_ids = player_known.known_fact_ids_sorted();
    let targets = [NpcRelationshipTarget::PlayerParty];
    let mut npc_speech_views: Vec<NpcSpeechProjection> = Vec::new();
    for npc_id in active_npc_ids {
        let profile = match db.load_npc_profile(session_id, npc_id).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                tracing::debug!(npc_id = %npc_id, "verifier_private_view: no durable profile, skipping NPC");
                continue;
            }
            Err(err) => {
                tracing::warn!(error = %err, npc_id = %npc_id, "verifier_private_view: profile load failed, skipping NPC");
                continue;
            }
        };
        match project_for_npc_speech(
            db,
            session_id,
            npc_id,
            &profile,
            &targets,
            &player_known_fact_ids,
        )
        .await
        {
            Ok(proj) => npc_speech_views.push(proj),
            Err(err) => {
                tracing::warn!(error = %err, npc_id = %npc_id, "verifier_private_view: speech projection failed, skipping NPC");
            }
        }
    }
    VerifierPrivateView::new(player_known, npc_speech_views)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_projection::npc_speech_projection_from_mind;
    use trpg_model::{KnowledgeState, NpcKnowledgeEntry, NpcMindView, NpcProfile};

    fn mind(npc_id: &str, entries: &[NpcKnowledgeEntry]) -> NpcMindView {
        let profile = NpcProfile {
            actor_id: npc_id.into(),
            name: npc_id.into(),
            ..Default::default()
        };
        NpcMindView::build("s", npc_id, &profile, &[], entries).unwrap()
    }

    /// 纯构造：玩家已知集进 player_known 视图，NPC 言谈投影逐个保留。
    #[test]
    fn pure_construction_holds_player_known_and_npc_views() {
        let player = PlayerNarrationProjection::from_player_known(["fact_known"]);
        let npc = npc_speech_projection_from_mind(
            mind(
                "npc_a",
                &[NpcKnowledgeEntry {
                    fact_id: "npc_secret".into(),
                    state: KnowledgeState::KnowsTrue,
                }],
            ),
            &[],
        );
        let view = VerifierPrivateView::new(player, vec![npc]);
        assert!(view.player_known.allows("fact_known"));
        assert!(
            !view.player_known.allows("npc_secret"),
            "玩家未知 fact 绝不进玩家叙事投影"
        );
        assert_eq!(view.npc_speech_views.len(), 1);
        assert_eq!(view.npc_speech_views[0].npc_id, "npc_a");
    }

    /// player_known downgrade：NPC 自有 known fact 绝不被当作玩家可见真相泄漏到 player_known。
    #[test]
    fn player_known_never_leaks_npc_hidden_truth() {
        // 玩家什么都不知道，但 NPC 知道一个隐藏真相。
        let player = PlayerNarrationProjection::from_player_known(Vec::<String>::new());
        let npc = npc_speech_projection_from_mind(
            mind(
                "npc_a",
                &[NpcKnowledgeEntry {
                    fact_id: "hidden_truth".into(),
                    state: KnowledgeState::KnowsTrue,
                }],
            ),
            &[],
        );
        let view = VerifierPrivateView::new(player, vec![npc]);
        // 玩家叙事投影绝不含 NPC 私有真相。
        assert!(!view.player_known.allows("hidden_truth"));
        assert!(view.player_known.known_fact_ids_sorted().is_empty());
        // 该真相只活在 NPC 自有投影里（玩家未知 → withheld，不可披露）。
        let proj = &view.npc_speech_views[0];
        assert_eq!(proj.known_fact_ids(), vec!["hidden_truth"]);
        assert!(
            !proj
                .revealable_fact_ids()
                .contains(&"hidden_truth".to_string()),
            "玩家未知的 NPC 已知真相默认 withheld，绝不可披露"
        );
    }

    /// 不变量编译级守卫：`VerifierPrivateView` 既不 `Serialize`、也无任何 `to_block`/render 方法。
    /// 这里只能正向断言它仍是 `Clone + Debug`（无渲染入口）；反向「无 to_block」由
    /// `prompt_safety_invariant_source_guard` grep 级测试 + 模块文档共同锁定。
    #[test]
    fn view_is_clone_debug_only_no_render_path() {
        fn assert_clone_debug<T: Clone + std::fmt::Debug>() {}
        assert_clone_debug::<VerifierPrivateView>();
    }

    /// grep 级守卫：本模块源码不得出现任何把私有视图渲染进 prompt 的入口
    /// （`to_block` / `render_` / `into_prompt` / `to_context_block` / `derive(Serialize)`）。
    /// 一旦有人误加渲染路径，此测试立刻红——配合模块文档不变量双保险。
    #[test]
    fn prompt_safety_invariant_source_guard() {
        let src = include_str!("verifier_private_view.rs");
        // 危险 token 列表按 ("fn", suffix) 二段存放并在用时拼接，故列表本身**不**含完整
        // 子串（否则守卫会扫到自己的检查项而误报）。任何真实新增的渲染方法会拼出完整
        // 子串并触发断言。
        for suffix in ["to_block", "render_", "into_prompt", "to_context_block"] {
            let bad = format!("fn {suffix}");
            assert!(
                !src.contains(&bad),
                "VerifierPrivateView 绝不可提供渲染进 prompt 的入口：发现 `{bad}`"
            );
        }
        // 结构体不得派生 Serialize（私有视图绝不序列化进任何 prompt/artifact 载荷）。
        let derive_token = ["#[derive(Debug, Clone,", " Serialize)]"].concat();
        assert!(
            !src.contains(&derive_token),
            "VerifierPrivateView 绝不可派生 Serialize"
        );
    }
}
