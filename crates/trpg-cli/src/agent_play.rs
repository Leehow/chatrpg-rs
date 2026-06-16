// crates/trpg-cli/src/agent_play.rs
//
// CLI play 会话循环：drain 统一 execute_turn 的 TurnEvent 流，同步到底。
// Delta → stdout 逐 token；AwaitingPlayerRoll / SceneTransition / Errata → 打印；
// TurnComplete → 更新 history + recent。scene 导航 / errata / carryover 全由
// execute_turn 内部的 postprocess phases 处理，CLI 不再手动调 scene_navigator。
use anyhow::Result;
use futures_util::StreamExt;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::Arc;
use trpg_gm::{
    execute_turn, GmLoop, LoopConfig, OwnedTurnRequest, SceneDeepExtractFn, ToolRegistry,
    TurnEvent, TurnOutcome, CANONICAL_TURN_PLAN,
};
use trpg_llm::LlmClient;
use trpg_model::{ChatMessage, ContextRequest, RuntimeState, TokenBudget, VisibilityProfile};
use trpg_runtime::scene_navigation::extract_module_scenes;
use trpg_runtime::RuntimeEngine;

pub async fn smoke_agent_play_symbol_exists() -> Result<()> {
    Ok(())
}

/// CLI play 会话主循环（永走 execute_turn，无 `--agent` flag）。
/// 内部自建 db/llm/search/engine，每回合重建 GmLoop 后 move 进 execute_turn。
pub async fn play_cli_agent(ruleset: &str, module: Option<&str>) -> Result<()> {
    let db = crate::connect_db().await?;
    db.migrate().await?;
    // make_llm() 已返回 Arc<dyn LlmClient>：直接持有 Arc，绝不从 &dyn 造 Arc。
    let llm = crate::make_llm()?;
    let data_dir = crate::default_data_dir();
    // 真 session bootstrap（绝不自造 session_id 字符串）：sessions 行落库 +
    // 模组入口场景激活（entry_node_id → current_scene_id）。
    let engine = RuntimeEngine::new(db.clone()).with_search(crate::make_search(&db, data_dir.clone())?);
    let session_id = engine.start_session(ruleset, module).await?;
    println!("session: {session_id}");
    println!("Type /quit to exit.");

    let mut history: Vec<ChatMessage> = Vec::new();
    let mut recent: Option<String> = None;

    loop {
        print!("\n[chatrpg]> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim().to_string();
        if input.is_empty() { continue; }
        if input == "/quit" { break; }

        // execute_turn 消费 GmLoop（owned，spawn 进 tokio 任务需 'static）。GmLoop 含
        // ToolRegistry(Box<dyn GmTool>)/ErrataMemory/ObligationLedger 等不可 Clone 字段，
        // 故每回合重建（RuntimeEngine 持 Arc<Db> 克隆，成本极低）。errata/obligations 是
        // per-turn 状态，TurnPipeline 内全程持有、TurnComplete 后随任务丢弃，不跨回合。
        let engine = RuntimeEngine::new(db.clone()).with_search(crate::make_search(&db, data_dir.clone())?);
        let mut gm = GmLoop::new(engine, llm.clone(), ToolRegistry::standard(), LoopConfig::default(), data_dir.clone());
        // 装配到场深抽闭包（scene_navigate phase 切场景到达时深抽目标场景）。
        wire_scene_extractor(&mut gm, &db, &llm, ruleset, module, &data_dir);

        let turn_id = format!("turn_{}", uuid::Uuid::new_v4().simple());
        let request = ContextRequest {
            ruleset_id: ruleset.to_string(),
            module_id: module.map(str::to_string),
            session_id: session_id.clone(),
            turn_id,
            viewer: VisibilityProfile::gm(),
            token_budget: TokenBudget::default(),
        };
        // 每回合把持久化的 current_scene_id 填进 RuntimeState（对齐 prepare_turn_context
        // 单点约定；scene_navigate phase 切换后下一回合在此读到新场景）。
        let scene_id = db.load_session_scene(&session_id).await.ok().flatten();
        let state = RuntimeState {
            ruleset_id: ruleset.to_string(),
            module_id: module.map(str::to_string),
            scene_id,
            ..Default::default()
        };

        let req = OwnedTurnRequest {
            request,
            state,
            user_input: input.clone(),
            history: history.clone(),
            recent_transcript: recent.clone(),
            module_id: module.map(str::to_string),
            data_dir: data_dir.clone(),
        };

        // execute_turn 返回 ReceiverStream<TurnEvent>；CLI 同步 drain 到底。
        let mut stream = Box::pin(execute_turn(gm, req, CANONICAL_TURN_PLAN));
        let mut streamed = String::new();

        while let Some(event) = stream.next().await {
            match event {
                TurnEvent::Delta(delta) => {
                    print!("{delta}");
                    io::stdout().flush().ok();
                    streamed.push_str(&delta);
                }
                TurnEvent::AwaitingPlayerRoll { check_id, prompt_public } => {
                    // standalone 事件 = live 信号（terminal 记录由 TurnComplete 承载，避免双 emit）。
                    println!("\n[awaiting_player_roll] check_id={check_id}");
                    println!("{prompt_public}");
                }
                TurnEvent::SceneTransition { from, to, reason } => {
                    println!("\n[scene] {from} → {to}  ({reason})");
                }
                TurnEvent::Errata(entry) => {
                    println!("\n[errata] {}", entry.detail);
                }
                TurnEvent::PostprocessScheduled => {
                    // CLI 同步 drain，PostprocessScheduled 仅作可观测标记，不需特殊处理。
                }
                TurnEvent::HeavyPostprocessDone => {
                    // play 模式：heavy 后台完成信号。已在 TurnComplete 处 break，
                    // 此分支理论上不可达；若因竞态先到此，直接忽略继续 drain。
                }
                TurnEvent::TurnFailed { phase, message, .. } => {
                    // obs T2/T6（spec §4.5）：play 模式遇阶段失败 → 红字一行提示，**不崩 shell**
                    // （fail-closed 不伪装成功，但交互式会话继续，玩家可重试）。失败终态，break。
                    println!("\n\x1b[31m[turn failed] phase={phase}: {message}\x1b[0m");
                    break;
                }
                TurnEvent::TurnWarning { phase, message } => {
                    println!("\n\x1b[33m[turn warning] phase={phase}: {message}\x1b[0m");
                }
                TurnEvent::TurnComplete { outcome } => {
                    println!();
                    match outcome {
                        TurnOutcome::Narration(text) => {
                            history.push(ChatMessage { role: "user".to_string(), content: input.clone() });
                            history.push(ChatMessage { role: "assistant".to_string(), content: text.clone() });
                            let tail = format!("\nPlayer: {input}\nGM: {text}\n");
                            let r = recent.get_or_insert_with(String::new);
                            r.push_str(&tail);
                            // 保持最近 12K 字符（与旧 play_cli take_tail_chars 对齐，按 char 边界裁剪）。
                            *r = take_tail_chars(r, 12_000);
                        }
                        TurnOutcome::AwaitingPlayerRoll { prompt_public, .. } => {
                            history.push(ChatMessage { role: "user".to_string(), content: input.clone() });
                            history.push(ChatMessage {
                                role: "assistant".to_string(),
                                content: if streamed.trim().is_empty() { prompt_public } else { streamed.clone() },
                            });
                        }
                    }
                    // R5 T3：play 模式不等 heavy（下一轮高水位守卫兜正确性）。
                    break;
                }
            }
        }
    }
    Ok(())
}

/// 装配到场深抽闭包（同 T4 execute_turn 内部约定，用于 scene_navigate phase）。
fn wire_scene_extractor(
    gm: &mut GmLoop,
    db: &trpg_db::Db,
    llm: &Arc<dyn LlmClient>,
    ruleset: &str,
    module: Option<&str>,
    data_dir: &std::path::Path,
) {
    if let Some(mid) = module.map(str::to_string) {
        let db2 = db.clone();
        let llm2 = llm.clone();
        let rs2 = ruleset.to_string();
        let dir2 = data_dir.to_path_buf();
        gm.scene_extractor = Some(Arc::new(move |node_id: String| {
            let db3 = db2.clone();
            let llm3 = llm2.clone();
            let mid3 = mid.clone();
            let rs3 = rs2.clone();
            let dir3 = dir2.clone();
            Box::pin(async move {
                extract_module_scenes(&db3, llm3.as_ref(), &mid3, None, Some(&rs3), &dir3, 12, Some(&node_id)).await
            }) as Pin<Box<dyn Future<Output = Result<usize>> + Send>>
        }) as SceneDeepExtractFn);
    }
}

/// 按 char 边界保留尾部 max_chars（与 main.rs take_tail_chars 同语义；CLI 内联避免跨模块可见性）。
fn take_tail_chars(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    input.chars().skip(count - max_chars).collect()
}
