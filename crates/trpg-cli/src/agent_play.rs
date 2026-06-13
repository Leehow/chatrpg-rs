// crates/trpg-cli/src/agent_play.rs
use anyhow::Result;
use serde_json::json;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::Arc;
use trpg_api::extract_module_scenes;
use trpg_gm::{GmLoop, GmTurnInput, LoopConfig, SceneDeepExtractFn, ToolRegistry, TurnOutcome};
use trpg_model::{ChatMessage, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::RuntimeEngine;

pub async fn smoke_agent_play_symbol_exists() -> Result<()> {
    Ok(())
}

/// --agent 会话循环。装配逐项对齐 play_cli：内部自建 db/llm/search/engine，
/// 不从外部收 `&dyn LlmClient`（不存在 From<&dyn> for Arc<dyn>，引用变不了所有权）。
pub async fn play_cli_agent(ruleset: &str, module: Option<&str>) -> Result<()> {
    let db = crate::connect_db().await?;
    db.migrate().await?;
    // make_llm() 已返回 Arc<dyn LlmClient>：直接持有 Arc，绝不从 &dyn 造 Arc。
    let llm = crate::make_llm()?;
    let data_dir = crate::default_data_dir();
    // with_search 接入后 retrieve_rules 工具才可用（否则恒 "retrieve unavailable: no search service"）。
    let search = crate::make_search(&db, data_dir.clone())?;
    let engine = RuntimeEngine::new(db.clone()).with_search(search);
    // 真 session bootstrap（绝不自造 session_id 字符串）：sessions 行落库 +
    // 模组入口场景激活（entry_node_id → current_scene_id）。record_world_event /
    // insert_pending_check / save_memory_event 的 FK、BP2 当前可玩单元投影、
    // navigate_scene 的 current 比对全靠它。
    let session_id = engine.start_session(ruleset, module).await?;
    println!("agent session: {session_id}");
    println!("Type /quit to exit.");
    // 一期固定 Text（StreamFormat 私有类型，同 crate 子模块可见）。
    let format = crate::StreamFormat::Text;
    let mut gm = GmLoop::new(engine, llm.clone(), ToolRegistry::standard(), LoopConfig::default(), data_dir.clone());
    if let Some(mid) = module.map(str::to_string) {
        let db2 = db.clone();
        let llm2 = llm.clone();
        let rs2 = ruleset.to_string();
        let dir2 = data_dir.clone();
        gm.scene_extractor = Some(Arc::new(move |node_id: String| {
            let db3 = db2.clone();
            let llm3 = llm2.clone();
            let mid3 = mid.clone();
            let rs3 = rs2.clone();
            let dir3 = dir2.clone();
            Box::pin(async move { extract_module_scenes(&db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id)).await })
                as Pin<Box<dyn Future<Output = Result<usize>> + Send>>
        }) as SceneDeepExtractFn);
    }
    let mut history: Vec<ChatMessage> = Vec::new();
    loop {
        print!("\n[chatrpg:agent]> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 { break; }
        let input = line.trim().to_string();
        if input.is_empty() { continue; }
        if input == "/quit" { break; }
        let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
        let request = ContextRequest { ruleset_id: ruleset.to_string(), module_id: module.map(str::to_string), session_id: session_id.clone(), turn_id, viewer: VisibilityProfile::gm(), token_budget: TokenBudget::default() };
        // 每回合把持久化的 current_scene_id 填进 RuntimeState（对齐 prepare_turn_context
        // 单点约定；scene_navigator 切换后下一回合在此读到新场景）。
        let scene_id = db.load_session_scene(&session_id).await.ok().flatten();
        let state = RuntimeState { ruleset_id: ruleset.to_string(), module_id: module.map(str::to_string), scene_id, ..Default::default() };
        let mut streamed = String::new();
        let outcome = gm.run_gm_turn(
            GmTurnInput { request: &request, state: &state, user_input: &input, history: &history, recent_transcript: None },
            &mut |delta| {
                streamed.push_str(delta);
                let _ = crate::emit_delta(format, delta);
            },
        ).await?;
        println!();
        let do_scene_nav = matches!(outcome, TurnOutcome::Narration(_));
        match outcome {
            TurnOutcome::Narration(text) => {
                history.push(ChatMessage { role: "user".to_string(), content: input });
                history.push(ChatMessage { role: "assistant".to_string(), content: text });
            }
            TurnOutcome::AwaitingPlayerRoll { prompt_public, .. } => {
                // emit_phase 第三参按值收 Value（owned json!），Result 用 let _ 接住。
                let _ = crate::emit_phase(format, "awaiting_player_roll", json!({"prompt_public": prompt_public.clone()}));
                history.push(ChatMessage { role: "user".to_string(), content: input });
                history.push(ChatMessage { role: "assistant".to_string(), content: if streamed.trim().is_empty() { prompt_public } else { streamed.clone() } });
            }
        }
        // 模组场景导航：比照 main.rs play_cli 的回合末处理，语义判定本回合叙事后是否切场景，
        // 更新 current_scene_id + 到场深抽。仅 Narration 终态触发；AwaitingPlayerRoll 跳过
        // （GM 在等玩家掷骰，场景未结束）。fail-closed：失败仅 warn 不阻断回合。
        if do_scene_nav {
            if let Some(mid) = module {
                if let Err(err) = trpg_api::scene_navigator(&db, llm.as_ref(), &session_id, mid, data_dir.as_path(), &streamed).await {
                    tracing::warn!("agent scene_navigator: {err:#}");
                }
            }
        }
    }
    Ok(())
}
