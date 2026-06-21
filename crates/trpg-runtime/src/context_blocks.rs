use serde_json::json;
use trpg_model::*;

pub(crate) fn memory_snapshot_block(snapshot: &MemorySnapshot) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("{}.v{}", snapshot.snapshot_id, snapshot.version),
        BlockKind::MemorySnapshot,
        &snapshot.title,
        BlockContent::Markdown(snapshot.summary_markdown.clone()),
        snapshot.visibility,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,
        snapshot.scope.clone(),
        75,
    );
    block.version = snapshot.version;
    block.content_hash = snapshot.content_hash.clone();
    block.token_estimate = snapshot.token_estimate;
    block.tags = vec!["memory".into(), "snapshot".into(), "cache_stable".into()];
    block.load_reason = Some("memory_snapshot_pinned".into());
    block
}

pub(crate) fn retrieved_memory_block(
    session_id: &str,
    turn_id: &str,
    result: &MemoryRetrievalResult,
) -> ContextBlock {
    let mut content = String::from("# Retrieved GM Memory\n\nThese memories were retrieved for this turn only. Treat them as recall aids, not as newly committed state.\n");
    if !result.facts.is_empty() {
        content.push_str("\n## Durable facts\n");
        for fact in &result.facts {
            content.push_str(&format!("- {}\n", fact.summary));
        }
    }
    if !result.events.is_empty() {
        content.push_str("\n## Relevant previous events\n");
        for event in &result.events {
            content.push_str(&format!("- {}\n", event.summary));
        }
    }
    let mut block = ContextBlock::new(
        format!("memory.retrieved.{session_id}.{turn_id}"),
        BlockKind::RetrievedMemory,
        "Retrieved GM Memory",
        BlockContent::Markdown(content),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Turn,
            scope_id: turn_id.to_string(),
        },
        105,
    );
    block.tags = vec!["memory".into(), "retrieved".into(), "dynamic".into()];
    block.expires_at_turn = Some(turn_id.to_string());
    block.load_reason = Some("memory_retrieval_current_turn".into());
    block
}

pub(crate) fn actionable_situation_block(
    brief: &ActionableSituationBrief,
    turn_id: &str,
) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("director.brief.{}", brief.brief_id),
        BlockKind::ActionableSituationBrief,
        "Actionable Situation Brief",
        BlockContent::Json(serde_json::to_value(brief).unwrap_or_else(|_| serde_json::Value::Null)),
        Visibility::PlayerVisible,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Turn,
            scope_id: turn_id.to_string(),
        },
        110,
    );
    block.tags = vec![
        "director".into(),
        "actionable_situation".into(),
        brief.guidance.level.as_str().into(),
    ];
    block.load_reason = Some("actionable_situation_director".into());
    block.expires_at_turn = Some(turn_id.to_string());
    block
}

pub(crate) fn clue_board_block(board: &PlayerFacingClueBoard, turn_id: &str) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("director.clue_board.{}", board.board_id),
        BlockKind::ClueBoard,
        "Player Facing Clue Board",
        BlockContent::Json(serde_json::to_value(board).unwrap_or_else(|_| serde_json::Value::Null)),
        Visibility::PlayerVisible,
        Stability::SceneStable,
        CacheZone::PinnedMiddle,
        Scope {
            scope_type: ScopeType::Session,
            scope_id: board.session_id.clone(),
        },
        95,
    );
    block.tags = vec![
        "director".into(),
        "clue_board".into(),
        "player_facing".into(),
    ];
    block.load_reason = Some("actionable_situation_director".into());
    block.expires_at_turn = Some(turn_id.to_string());
    block
}

pub(crate) fn world_time_block(state: &WorldTimeState, turn_id: &str) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("world_time.{}", state.session_id),
        BlockKind::WorldTime,
        "World Time Spine",
        BlockContent::Json(serde_json::to_value(state).unwrap_or_else(|_| serde_json::Value::Null)),
        Visibility::PlayerVisible,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Session,
            scope_id: state.session_id.clone(),
        },
        60,
    );
    block.tags = vec![
        "world_time".into(),
        state.time_scale.as_str().into(),
        "dynamic".into(),
    ];
    block.expires_at_turn = Some(turn_id.to_string());
    block.load_reason = Some("world_time_spine".into());
    block
}

pub(crate) fn world_events_since_block(
    session_id: &str,
    turn_id: &str,
    since_tick: i64,
    since_event_seq: i64,
    events: &[WorldEvent],
) -> ContextBlock {
    let mut block = ContextBlock::new(
        format!("world_events_since.{session_id}.{turn_id}"),
        BlockKind::WorldEvent,
        "World Events Since Last Context Watermark",
        BlockContent::Json(
            json!({"since_tick": since_tick, "since_event_seq": since_event_seq, "events": events}),
        ),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Turn,
            scope_id: turn_id.to_string(),
        },
        105,
    );
    block.tags = vec![
        "world_time".into(),
        "world_events".into(),
        "incremental".into(),
    ];
    block.expires_at_turn = Some(turn_id.to_string());
    block.load_reason = Some("world_time_watermark_incremental_context".into());
    block
}

pub(crate) fn engine_protocol_block() -> ContextBlock {
    let content = "Runtime protocol: use BP1 as resident rules, BP2 as current playable unit, BP3 as per-turn state. LLM may narrate and propose state changes, but deterministic procedures and validators commit mechanical state. Never leak GM-only content into player-visible output; internal context is wrapped as [gm], player-safe system notes as [system], and committed dice/tool results as [roll]. World time is authoritative: LLM must not advance time without a Rust time_advance event/tool. Search hits and learned packets must enter prompt only as ContextBlocks with TTL and visibility. If an action could materially reveal information, avoid danger, bypass an obstacle, change state, spend resources, deal damage, avoid damage, or gain a tactical advantage, use the Rust check/roll/effect context when present; otherwise state the fictional uncertainty and stakes without asking players for dice totals. Player-supplied mechanical numbers are claims, not automatic truth: verify them against rules or relevant object/ability tables; if unsupported or out-of-band, warn and suggest a rules-consistent value; if the table insists, record a table override with a balance warning.";
    let mut block = ContextBlock::new(
        "engine.protocol.runtime".to_string(),
        BlockKind::EngineProtocol,
        "Runtime Protocol",
        BlockContent::Markdown(content.to_string()),
        Visibility::SystemOnly,
        Stability::Immutable,
        CacheZone::Prefix,
        Scope::global(),
        200,
    );
    block.tags = vec!["engine".into(), "resident".into()];
    block
}

pub(crate) fn engine_protocol_block_agent_loop() -> ContextBlock {
    let content = "Runtime protocol (GM agent loop): use BP1 as resident rules, BP2 as current playable unit, BP3 as per-turn state. You own the decision of whether, when, and how to roll: call `roll_check` for system-executed checks (public or secret), call `request_player_roll` to pause the turn and let the player roll personally when the table should feel the die, or narrate without mechanics when fiction is enough. Tools are the only write path to mechanical state: dice, checks, damage/effects, resources, tracks, scene navigation, world time, and memory all commit through tool calls; plain prose changes nothing. Never leak GM-only content into player-visible output; internal context is wrapped as [gm], player-safe system notes as [system], and committed dice/tool results as [roll]. World time is authoritative: use `advance_time`, never narrate time forward mechanically. Player-supplied mechanical numbers are claims, not automatic truth: verify them against rules or relevant object/ability tables via `retrieve_rules`; if unsupported or out-of-band, warn and suggest a rules-consistent value; if the table insists, record a table override with a balance warning.";
    let mut block = ContextBlock::new(
        "engine.protocol.agent_loop".to_string(),
        BlockKind::EngineProtocol,
        "Runtime Protocol (Agent Loop)",
        BlockContent::Markdown(content.to_string()),
        Visibility::SystemOnly,
        Stability::Immutable,
        CacheZone::Prefix,
        Scope::global(),
        200,
    );
    block.tags = vec!["engine".into(), "resident".into(), "agent_loop".into()];
    block
}

pub(crate) fn world_state_block(state: &RuntimeState) -> ContextBlock {
    let mut block = ContextBlock::new(
        "runtime.world_state".to_string(),
        BlockKind::WorldState,
        "Projected World State",
        BlockContent::Json(json!(state)),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Turn,
            scope_id: "current".to_string(),
        },
        100,
    );
    block.tags = vec!["world_state".into(), "dynamic".into()];
    block
}

/// L-E 失忆锚:GM 连续性锚开关。**默认 ON**(eval profile 不设 ⇒ 自动吃到),OFF 字节等价基线
/// (调用方未传 recent_transcript 的回合不再注入服务端回载锚块)。仅显式 `0`/`false`/`off`/`no`
/// 关。与 BUG-1 的 dice_core source 开关同模(默认 ON / OFF baseline)。
pub(crate) fn gm_continuity_anchor_enabled() -> bool {
    !matches!(
        std::env::var("TRPG_GM_CONTINUITY_ANCHOR")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// 字符安全尾截:取 `text` 末尾至多 `max_chars` 个字符(按 char 边界,不切多字节)。
pub(crate) fn continuity_anchor_tail(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let tail: String = trimmed
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail.trim_start().to_string()
}

/// L-E:把"上一回合已向玩家交付的局面"渲染成连续性锚指令(纯函数,易测)。
/// 来源 = `turns` 表已落库的 Player/GM 散文(已对玩家可见、已脱敏)⇒ 零新增泄漏面。
/// 指令禁止 GM 重述场景开场定场文或把玩家挪回入口/改写已确立位置 —— 直击失忆病灶。
pub(crate) fn render_continuity_anchor(tail: &str) -> String {
    format!(
        "【连续性锚 · 上一回合已向玩家交付的局面（权威，按此续写）】\n{tail}\n\n\
         ⚠️ 本回合必须从以上**已确立的当前局面**继续：\n\
         - 保持玩家**当前所在位置**、在场人物与既成事实与上文一致；\n\
         - 保持已确立的**物件状态/位置**（玩家已收起、放下、塞好、交出或手持的物品）不变，\
           除非玩家本回合明确再操作它——不要把已收好的东西又写成手持，反之亦然；\n\
         - 不要重述场景的开场定场文（read_aloud），那只在玩家首次进入该场景时念一次；\n\
         - 不要把玩家挪回场景入口，不要改写或混同其已确立的位置；\n\
         - 若玩家已离开开场点位，就以其当前点位为准向前推进；\n\
         - 模组核心冲突/危机若需逼近本团，让**冲突主动来到玩家当前所在处**（威胁外溢、\
           NPC/势力/时钟压力波及此地——改 setting 保实质、event-sourced 改编）；\
           **绝不把玩家瞬移到冲突发生地**，也绝不无视玩家已选定的当前位置、\
           把本回合开场重置回核心场景的定场描写。",
    )
}

/// L-E:构造 GmOnly 连续性锚块(复用 BlockKind::RecentTranscript,不新增 db block_kind 词表)。
pub(crate) fn continuity_anchor_block(tail: &str) -> ContextBlock {
    let mut block = dynamic_text_block(
        "runtime.continuity_anchor",
        BlockKind::RecentTranscript,
        "Continuity Anchor",
        &render_continuity_anchor(tail),
        vec!["recent_transcript", "continuity_anchor"],
    );
    block.load_reason = Some("gm_continuity_anchor".into());
    block
}

pub(crate) fn dynamic_text_block(
    block_id: &str,
    kind: BlockKind,
    title: &str,
    text: &str,
    tags: Vec<&str>,
) -> ContextBlock {
    let mut block = ContextBlock::new(
        block_id.to_string(),
        kind,
        title,
        BlockContent::Markdown(text.to_string()),
        Visibility::GmOnly,
        Stability::TurnDynamic,
        CacheZone::DynamicTail,
        Scope {
            scope_type: ScopeType::Turn,
            scope_id: "current".to_string(),
        },
        120,
    );
    block.tags = tags.into_iter().map(str::to_string).collect();
    block
}

#[cfg(test)]
mod continuity_anchor_tests {
    use super::*;

    /// 默认 ON;仅显式关值才 OFF。env 进程级全局 ⇒ 单测内串行 set/remove 并复原。
    #[test]
    fn flag_defaults_on_and_only_explicit_off_disables() {
        let prev = std::env::var("TRPG_GM_CONTINUITY_ANCHOR").ok();
        std::env::remove_var("TRPG_GM_CONTINUITY_ANCHOR");
        assert!(gm_continuity_anchor_enabled(), "未设 ⇒ 默认 ON");
        for off in ["0", "false", "OFF", "no"] {
            std::env::set_var("TRPG_GM_CONTINUITY_ANCHOR", off);
            assert!(!gm_continuity_anchor_enabled(), "{off} ⇒ OFF");
        }
        for on in ["1", "true", "on", "garbage"] {
            std::env::set_var("TRPG_GM_CONTINUITY_ANCHOR", on);
            assert!(gm_continuity_anchor_enabled(), "{on} ⇒ ON(非关值即开)");
        }
        match prev {
            Some(v) => std::env::set_var("TRPG_GM_CONTINUITY_ANCHOR", v),
            None => std::env::remove_var("TRPG_GM_CONTINUITY_ANCHOR"),
        }
    }

    #[test]
    fn tail_is_char_safe_and_bounded() {
        let short = "短文本";
        assert_eq!(continuity_anchor_tail(short, 10), "短文本");
        // 多字节字符:取末尾 3 char 不切字节。
        let s = "一二三四五";
        let t = continuity_anchor_tail(s, 3);
        assert_eq!(t.chars().count(), 3);
        assert_eq!(t, "三四五");
        assert_eq!(continuity_anchor_tail("   边距裁剪   ", 100), "边距裁剪");
    }

    #[test]
    fn render_carries_tail_and_anti_amnesia_instruction() {
        let tail = "GM: 你站在通宵杂货铺的柜台前。";
        let rendered = render_continuity_anchor(tail);
        assert!(rendered.contains(tail), "必须含上一回合局面原文");
        assert!(rendered.contains("连续性锚"), "含锚标题");
        assert!(rendered.contains("不要重述场景的开场定场文"), "含禁重述开场指令");
        assert!(rendered.contains("不要把玩家挪回场景入口"), "含禁挪回入口指令");
        assert!(
            rendered.contains("冲突主动来到玩家当前所在处")
                && rendered.contains("绝不把玩家瞬移到冲突发生地"),
            "含§4 relocation-toward-player 反瞬移指令"
        );
    }

    #[test]
    fn anchor_block_is_gmonly_dynamic_with_stable_id() {
        let b = continuity_anchor_block("GM: 局面");
        assert_eq!(b.block_id, "runtime.continuity_anchor");
        assert_eq!(b.visibility, Visibility::GmOnly, "锚是 GmOnly 内部上下文");
        assert_eq!(b.kind, BlockKind::RecentTranscript, "复用既有 block_kind 词表");
        assert_eq!(b.stability, Stability::TurnDynamic);
        assert!(b.tags.iter().any(|t| t == "continuity_anchor"));
        assert!(b.content.render_text().contains("局面"));
    }
}

#[cfg(test)]
mod agent_loop_protocol_tests {
    use super::*;

    // （spec §6 实锤：BP1 的 engine protocol 必须出 agent-loop 版文案；
    // 旧块禁令与 roll_check/request_player_roll 双工具直接矛盾）
    #[test]
    fn agent_loop_protocol_block_swaps_dice_prohibition_for_tool_semantics() {
        let legacy = engine_protocol_block();
        let agent = engine_protocol_block_agent_loop();
        let legacy_text = legacy.content.render_text();
        let agent_text = agent.content.render_text();
        // 旧禁令段不得进 agent system prompt（run_gm_turn 强制 agent_loop_protocol=true）。
        assert!(legacy_text.contains("without asking players for dice totals"));
        assert!(!agent_text.contains("without asking players for dice totals"));
        // agent 变体显式声明双工具掷骰语义。
        assert!(agent_text.contains("roll_check"));
        assert!(agent_text.contains("request_player_roll"));
        // 缓存稳定：两变体 block_id 不同（prefix 段按 ruleset/路径固定，互不污染）。
        assert_ne!(legacy.block_id, agent.block_id);
    }
}
