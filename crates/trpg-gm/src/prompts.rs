use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use trpg_llm::AggregatedToolCall;
use trpg_model::{ChatMessage, CompiledContext, ContextRequest};

/// dynamic tail 输入（§6.1 第 1 条：最易变段排最后）。
pub struct DynamicTailInput<'a> {
    pub user_input: &'a str,
    /// 确定性头部 gate 结算产出的「已发生事实」（[roll]…[/roll] 渲染文本）；无则空。
    pub resolved_gate_facts: &'a [String],
    /// ErrataMemory 的勘误块 + 持续提醒块；无则空（块整体缺省，不写空块）。
    pub errata_blocks: &'a [String],
    /// 上回合遗留机械债务（ObligationLedger::carryover_block）；排在
    /// errata_blocks 之后、Player Input 之前。None ⇒ 不写块。
    pub obligations_block: Option<&'a str>,
    /// 活动 NPC 行为指引（TC-D3-02）：每个活动 NPC 的 prompt-safe 行为块拼接，
    /// 排在 obligations_block 之后、Player Input 之前。None/空 ⇒ 不写块（无活动
    /// NPC 的回合字节不变，缓存稳定）。仅含安全 persona 投影 + reveal/withhold fact id，
    /// 绝无 GM-only secret 文本或玩家未知 fact 散文。
    pub npc_guidance_block: Option<&'a str>,
    /// P5.6 Director brief packet (`[director_packet]…[/director_packet]`): GM-only decision
    /// scaffolding rendered from the typed [`trpg_model::DirectorPlan`]. Appended after
    /// `npc_guidance_block`, before Player Input — same `[gm]` BP3 message, NEVER the
    /// player-visible narration. None/empty ⇒ no block (flag OFF ⇒ byte-identical baseline).
    /// Carries only ids / enum tokens / short structural strings (content-safety owned by the
    /// renderer, since this is a GM-tail string that bypasses the ContextFilter).
    pub director_packet_block: Option<&'a str>,
}

/// 回合消息容器：assemble 渲染一次、整回合复用；工具轮只在尾部 push，
/// 绝不重渲染/重排任何前缀字节（§6.1 第 2 条）。
pub struct TurnMessages {
    messages: Vec<Value>,
    assembled_len: usize,
}

impl TurnMessages {
    /// §6.1 布局（稳定性降序）：system(BP1+skill) → user(BP2) → history… → user(BP3+尾段)。
    pub fn assemble(
        compiled: &CompiledContext,
        gm_skill_text: &str,
        history: &[ChatMessage],
        tail: &DynamicTailInput<'_>,
    ) -> Self {
        let mut messages = Vec::new();
        // Q-6 (§2b.2 referee, not yes-man): append GM-craft adjudication overlay when
        // TRPG_GM_CRAFT ON; OFF ⇒ byte-identical baseline system message.
        let system_content = crate::gm_craft::adjudicator_system(
            format!("{}\n\n{}", compiled.prefix_text, gm_skill_text),
            crate::gm_craft::enabled(),
        );
        messages.push(json!({"role":"system","content": system_content}));
        messages.push(json!({"role":"user","content": format!("[gm]\n[BP2: Pinned Context]\n{}\n[/gm]", compiled.pinned_text)}));
        for h in history {
            messages.push(json!({"role": h.role, "content": h.content}));
        }
        let mut dynamic = format!(
            "[gm]\n[BP3: Dynamic Context]\n{}\n[/gm]",
            compiled.dynamic_text
        );
        // fail-closed 提示（不含任何 ruleset 内容）：BP2 pinned 投影为空 ⇒ 模型
        // 看不到已绑定 PC 卡，提醒其先 get_actor 再裁定（深层投影修复留二期）。
        if compiled.pinned_text.trim().is_empty() {
            dynamic.push_str("\n\n[system] No character projection is present in the pinned context. Call get_actor (e.g. actor_id \"pc.current\") to load the bound character sheet before any mechanical adjudication.");
        }
        if !tail.resolved_gate_facts.is_empty() {
            dynamic.push_str("\n\n[Resolved Gate Facts]\n");
            dynamic.push_str(&tail.resolved_gate_facts.join("\n"));
        }
        if !tail.errata_blocks.is_empty() {
            dynamic.push_str("\n\n");
            dynamic.push_str(&tail.errata_blocks.join("\n\n"));
        }
        if let Some(block) = tail.obligations_block {
            dynamic.push_str("\n\n");
            dynamic.push_str(block);
        }
        if let Some(block) = tail.npc_guidance_block.filter(|b| !b.trim().is_empty()) {
            dynamic.push_str("\n\n");
            dynamic.push_str(block);
        }
        // P5.6 Director brief packet — mirrors npc_guidance_block exactly (same [gm] BP3
        // message, Option::filter empty). flag OFF ⇒ None ⇒ nothing appended ⇒ byte-identical.
        if let Some(block) = tail.director_packet_block.filter(|b| !b.trim().is_empty()) {
            dynamic.push_str("\n\n");
            dynamic.push_str(block);
        }
        dynamic.push_str("\n\n[Player Input]\n");
        dynamic.push_str(tail.user_input);
        messages.push(json!({"role":"user","content": dynamic}));
        let assembled_len = messages.len();
        Self {
            messages,
            assembled_len,
        }
    }

    /// 工具轮：追加 assistant tool_calls 消息（id/name/arguments 原样回放）。
    pub fn push_assistant_tool_calls(&mut self, calls: &[AggregatedToolCall]) {
        let tool_calls = calls.iter().map(|c| json!({"id": c.id, "type":"function", "function":{"name": c.name, "arguments": c.arguments}})).collect::<Vec<_>>();
        self.messages
            .push(json!({"role":"assistant","content": Value::Null,"tool_calls": tool_calls}));
    }

    /// 工具轮：追加一条 tool role 结果消息（content = dispatch 产物）。
    pub fn push_tool_result(&mut self, tool_call_id: &str, name: &str, content: &str) {
        self.messages.push(
            json!({"role":"tool","tool_call_id": tool_call_id,"name": name,"content": content}),
        );
    }

    /// 债务门控（B6）：被拦下的叙事终态轮回填 block_text() 为 system 观察，
    /// 该轮 content 整体丢弃后循环继续（尾部追加，前缀字节不动）。
    pub fn push_system_observation(&mut self, text: &str) {
        self.messages.push(json!({"role":"system","content": text}));
    }

    /// 每轮请求体快照（Vec<Value> clone）。
    pub fn to_request_messages(&self) -> Vec<Value> {
        self.messages.clone()
    }

    /// P5 revision (Gap 3): a DIRECT, deterministic packet-presence signal on the ACTUAL
    /// assembled messages — true iff the `[director_packet]` marker was folded into the GM
    /// BP3 tail. Unlike `bp3_hash` (= `compiled.dynamic_hash`, computed at compile BEFORE the
    /// packet tail is appended in `assemble`), this inspects the post-assemble message text,
    /// so it legitimately proves the packet entered the GM prompt. OFF / empty packet ⇒ false
    /// (the tail is filtered out → byte-identical baseline). Scoped, side-effect-free.
    pub fn contains_director_packet(&self) -> bool {
        self.messages.iter().any(|m| {
            m.get("content")
                .and_then(Value::as_str)
                .is_some_and(|c| c.contains("[director_packet]"))
        })
    }

    /// 缓存稳定可观测：前 first_n 条消息序列化字节的稳定哈希（Task 9 回归用）。
    /// first_n 截顶到 assemble 产出段长度：工具轮尾部追加的消息永不进哈希窗口。
    pub fn prefix_byte_hash(&self, first_n: usize) -> String {
        let n = first_n.min(self.assembled_len);
        let bytes = serde_json::to_vec(&self.messages.iter().take(n).collect::<Vec<_>>())
            .unwrap_or_default();
        let digest = Sha256::digest(bytes);
        format!("sha256:{digest:x}")
    }
}

fn sorted_md_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

/// gm_skill 装载：global/*.md（文件名字典序）→ <ruleset_id>/*.md 追加（两级合并）。
/// global 目录缺失 → Err（gm_skill 是 agent 路径硬依赖，fail-closed）。
pub fn load_gm_skill(data_dir: &Path, ruleset_id: &str) -> Result<String> {
    let base = data_dir.join("agent/gm_skill");
    let global = base.join("global");
    if !global.exists() {
        return Err(anyhow!(
            "global gm_skill directory missing: {}",
            global.display()
        ));
    }
    let mut chunks = Vec::new();
    for p in sorted_md_files(&global)? {
        chunks.push(
            fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?,
        );
    }
    let ruleset = base.join(ruleset_id);
    for p in sorted_md_files(&ruleset)? {
        chunks.push(
            fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?,
        );
    }
    Ok(chunks.join("\n\n---\n\n"))
}

/// 三期 §4.2 提示四级合并：global/*.md → <ruleset>/*.md → modes/<mode>/global.md
/// → modes/<mode>/<ruleset>.md（mode 层内部仍按 ruleset 覆盖）。mode=None 退化为
/// load_gm_skill 两级合并（**字节不变**——缓存稳定硬回归）。mode 提示 md 缺失则
/// 跳过（manifest.json 才是 mode 存在性的 fail-closed 锚点，且永不进提示文本）。
pub fn load_gm_skill_with_mode(
    data_dir: &Path,
    ruleset: &str,
    mode: Option<&str>,
) -> Result<String> {
    let mut text = load_gm_skill(data_dir, ruleset)?;
    let Some(mode) = mode else { return Ok(text) };
    let mode_dir = data_dir.join("agent/gm_skill/modes").join(mode);
    for file in ["global.md".to_string(), format!("{ruleset}.md")] {
        let path = mode_dir.join(&file);
        if !path.exists() {
            continue;
        }
        let chunk = fs::read_to_string(&path)
            .with_context(|| format!("failed reading {}", path.display()))?;
        text.push_str("\n\n---\n\n");
        text.push_str(&chunk);
    }
    Ok(text)
}

/// §6.1 第 4 条 fail-closed：prefix/pinned 段超 TokenBudget ⇒ 配置错误
/// （Err 信息含 "prefix"/"pinned" 段名），绝不静默裁剪。
pub fn validate_compiled_budget(
    compiled: &CompiledContext,
    request: &ContextRequest,
) -> Result<()> {
    let prefix_tokens = compiled.prefix_text.split_whitespace().count() as u32;
    let pinned_tokens = compiled.pinned_text.split_whitespace().count() as u32;
    if prefix_tokens > request.token_budget.prefix_max {
        return Err(anyhow!("prefix segment exceeds TokenBudget.prefix_max ({prefix_tokens} > {}); prefix/pinned over budget is a configuration error, never silently trimmed", request.token_budget.prefix_max));
    }
    if pinned_tokens > request.token_budget.pinned_max {
        return Err(anyhow!("pinned segment exceeds TokenBudget.pinned_max ({pinned_tokens} > {}); prefix/pinned over budget is a configuration error, never silently trimmed", request.token_budget.pinned_max));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use trpg_model::{ChatMessage, CompiledContext};

    fn compiled() -> CompiledContext {
        CompiledContext {
            prefix_text: "BP1".to_string(),
            pinned_text: "BP2".to_string(),
            dynamic_text: "BP3".to_string(),
            prefix_hash: "p".to_string(),
            pinned_hash: "m".to_string(),
            dynamic_hash: "d".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn assemble_order_is_stable() {
        let history = vec![ChatMessage {
            role: "assistant".to_string(),
            content: "old".to_string(),
        }];
        let tail = DynamicTailInput {
            user_input: "go",
            resolved_gate_facts: &["[roll]done[/roll]".to_string()],
            errata_blocks: &["[gm_errata]fix[/gm_errata]".to_string()],
            obligations_block: Some("[obligations_carryover]debt[/obligations_carryover]"),
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let messages = TurnMessages::assemble(&compiled(), "SKILL", &history, &tail);
        let raw = messages.to_request_messages();
        assert_eq!(raw[0].get("role").and_then(Value::as_str), Some("system"));
        assert!(raw[0]
            .get("content")
            .and_then(Value::as_str)
            .unwrap()
            .contains("BP1"));
        assert!(raw[1]
            .get("content")
            .and_then(Value::as_str)
            .unwrap()
            .contains("BP2"));
        assert!(raw[2]
            .get("content")
            .and_then(Value::as_str)
            .unwrap()
            .contains("old"));
        assert!(raw[3]
            .get("content")
            .and_then(Value::as_str)
            .unwrap()
            .contains("BP3"));
        assert!(raw[3]
            .get("content")
            .and_then(Value::as_str)
            .unwrap()
            .contains("[Player Input]"));
        // B6 契约：obligations_block 排在 errata_blocks 之后、Player Input 之前。
        let dynamic = raw[3].get("content").and_then(Value::as_str).unwrap();
        assert!(
            dynamic.find("[gm_errata]").unwrap() < dynamic.find("[obligations_carryover]").unwrap()
        );
        assert!(
            dynamic.find("[obligations_carryover]").unwrap()
                < dynamic.find("[Player Input]").unwrap()
        );
    }

    #[test]
    fn empty_pinned_context_injects_get_actor_hint() {
        // BP2 空投影兜底（e2e must-fix 便宜修法）：pinned 为空串 ⇒ dynamic tail
        // 注入 [system] get_actor 提示；非空 ⇒ 绝不注入（不污染正常回合）。
        let tail = DynamicTailInput {
            user_input: "go",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let mut empty_pinned = compiled();
        empty_pinned.pinned_text = "  ".to_string();
        let messages = TurnMessages::assemble(&empty_pinned, "SKILL", &[], &tail);
        let dynamic = messages
            .to_request_messages()
            .last()
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        assert!(
            dynamic.contains("[system] No character projection"),
            "missing fail-closed hint: {dynamic}"
        );
        assert!(dynamic.contains("get_actor"));
        let normal = TurnMessages::assemble(&compiled(), "SKILL", &[], &tail);
        let dynamic = normal
            .to_request_messages()
            .last()
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        assert!(!dynamic.contains("No character projection"));
    }

    #[test]
    fn push_does_not_change_prefix_hash() {
        let tail = DynamicTailInput {
            user_input: "go",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let mut messages = TurnMessages::assemble(&compiled(), "SKILL", &[], &tail);
        let before = messages.prefix_byte_hash(4);
        messages.push_tool_result("call_1", "retrieve_rules", "{\"ok\":true}");
        assert_eq!(before, messages.prefix_byte_hash(4));
    }

    #[test]
    fn gm_skill_merge_order_includes_new_entries() {
        // B8：global gm_skill 新增 40_mechanics_catalog / 50_obligation_policy 两条准则，
        // load_gm_skill 按文件名字典序合并 ⇒ 40_ 内容出现在 30_ 之后、50_ 在 40_ 之后。
        // 直接装载仓库真实 data/ 目录（ruleset 子目录不存在 ⇒ 仅 global 六份）。
        let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let text = load_gm_skill(&data_dir, "__no_such_ruleset__").unwrap();
        let p30 = text
            .find("Player-visible Output Contract")
            .expect("30_output_contract content missing");
        let p40 = text
            .find("目录优先于自由发挥")
            .expect("40_mechanics_catalog content missing");
        let p50 = text
            .find("due 必须回应")
            .expect("50_obligation_policy content missing");
        assert!(
            p30 < p40,
            "40_ content must appear after 30_ ({p30} vs {p40})"
        );
        assert!(
            p40 < p50,
            "50_ content must appear after 40_ ({p40} vs {p50})"
        );
    }

    #[test]
    fn gm_skill_with_mode_merges_four_levels_in_order() {
        // 三期 §4.2：global → ruleset → mode-global → mode-ruleset；
        // manifest.json 绝不进提示文本。
        let dir = std::env::temp_dir().join(format!(
            "gm_skill_mode_test_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(dir.join("agent/gm_skill/global")).unwrap();
        fs::create_dir_all(dir.join("agent/gm_skill/coc")).unwrap();
        fs::create_dir_all(dir.join("agent/gm_skill/modes/combat")).unwrap();
        fs::write(dir.join("agent/gm_skill/global/10_a.md"), "L1-global").unwrap();
        fs::write(dir.join("agent/gm_skill/coc/10_b.md"), "L2-ruleset").unwrap();
        fs::write(
            dir.join("agent/gm_skill/modes/combat/global.md"),
            "L3-mode-global",
        )
        .unwrap();
        fs::write(
            dir.join("agent/gm_skill/modes/combat/coc.md"),
            "L4-mode-ruleset",
        )
        .unwrap();
        fs::write(
            dir.join("agent/gm_skill/modes/combat/manifest.json"),
            r#"{"mode_id":"combat","frame_kind":"combat"}"#,
        )
        .unwrap();
        let text = load_gm_skill_with_mode(&dir, "coc", Some("combat")).unwrap();
        let p1 = text.find("L1-global").expect("L1 missing");
        let p2 = text.find("L2-ruleset").expect("L2 missing");
        let p3 = text.find("L3-mode-global").expect("L3 missing");
        let p4 = text.find("L4-mode-ruleset").expect("L4 missing");
        assert!(
            p1 < p2 && p2 < p3 && p3 < p4,
            "merge order broken: {p1}/{p2}/{p3}/{p4}"
        );
        assert!(
            !text.contains("mode_id"),
            "manifest.json must never enter the prompt: {text}"
        );
        // mode 提示文件缺失（另一 ruleset）→ 仅 mode-global 追加，不 Err。
        let other = load_gm_skill_with_mode(&dir, "__none__", Some("combat")).unwrap();
        assert!(other.contains("L3-mode-global") && !other.contains("L4-mode-ruleset"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn gm_skill_with_mode_none_is_byte_identical_to_two_level_merge() {
        // 硬验收：mode=None 路径与二期 load_gm_skill 字节级一致（仓库真实 data/，
        // modes/ 目录已存在也绝不影响 None 路径）。
        let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
        let legacy = load_gm_skill(&data_dir, "__no_such_ruleset__").unwrap();
        let with_mode = load_gm_skill_with_mode(&data_dir, "__no_such_ruleset__", None).unwrap();
        assert_eq!(legacy.as_bytes(), with_mode.as_bytes());
    }

    #[test]
    fn load_gm_skill_merges_global_then_ruleset() {
        let dir = std::env::temp_dir().join(format!("gm_skill_test_{}", std::process::id()));
        let global = dir.join("agent/gm_skill/global");
        let ruleset = dir.join("agent/gm_skill/cyberpunk_red");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&ruleset).unwrap();
        fs::write(global.join("10_a.md"), "global-a").unwrap();
        fs::write(global.join("20_b.md"), "global-b").unwrap();
        fs::write(ruleset.join("10_c.md"), "ruleset-c").unwrap();
        let text = load_gm_skill(&dir, "cyberpunk_red").unwrap();
        assert!(text.find("global-a").unwrap() < text.find("global-b").unwrap());
        assert!(text.find("global-b").unwrap() < text.find("ruleset-c").unwrap());
        fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod cache_stability_tests {
    use super::*;
    use trpg_llm::AggregatedToolCall;
    use trpg_model::{ChatMessage, CompiledContext, ContextRequest};

    fn compiled() -> CompiledContext {
        CompiledContext {
            prefix_text: "PFX".to_string(),
            pinned_text: "PIN".to_string(),
            dynamic_text: "DYN".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn same_inputs_assemble_same_bytes() {
        let tail = DynamicTailInput {
            user_input: "x",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let a = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        let b = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        assert_eq!(
            serde_json::to_vec(&a.to_request_messages()).unwrap(),
            serde_json::to_vec(&b.to_request_messages()).unwrap()
        );
    }

    #[test]
    fn tool_append_changes_only_tail() {
        let tail = DynamicTailInput {
            user_input: "x",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let mut m = TurnMessages::assemble(
            &compiled(),
            "skill",
            &[ChatMessage {
                role: "assistant".to_string(),
                content: "old".to_string(),
            }],
            &tail,
        );
        let before = m.to_request_messages();
        m.push_assistant_tool_calls(&[AggregatedToolCall {
            id: "c".to_string(),
            name: "remember".to_string(),
            arguments: "{}".to_string(),
        }]);
        let after = m.to_request_messages();
        assert_eq!(
            serde_json::to_vec(&before).unwrap(),
            serde_json::to_vec(&after[..before.len()]).unwrap()
        );
    }

    #[test]
    fn prefix_hash_is_byte_level_hash() {
        let tail = DynamicTailInput {
            user_input: "x",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let m = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        assert_eq!(m.prefix_byte_hash(2), m.prefix_byte_hash(2));
        assert_ne!(m.prefix_byte_hash(1), m.prefix_byte_hash(2));
    }

    #[test]
    fn cross_turn_stable_segments_keep_bytes_with_growing_history() {
        // §6.1 第 3 条 / spec §9 单测 9 的回归本体：场景不变、gm_skill 不变，
        // 跨回合 history 仅追加 ⇒ system+BP2 两条消息字节不变，旧 history 段不改写。
        let turn1_history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "scene".to_string(),
            },
        ];
        let tail1 = DynamicTailInput {
            user_input: "look",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let turn1 = TurnMessages::assemble(&compiled(), "skill", &turn1_history, &tail1);
        let mut turn2_history = turn1_history.clone();
        turn2_history.push(ChatMessage {
            role: "user".to_string(),
            content: "look".to_string(),
        });
        turn2_history.push(ChatMessage {
            role: "assistant".to_string(),
            content: "you see".to_string(),
        });
        let tail2 = DynamicTailInput {
            user_input: "move",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let turn2 = TurnMessages::assemble(&compiled(), "skill", &turn2_history, &tail2);
        let raw1 = turn1.to_request_messages();
        let raw2 = turn2.to_request_messages();
        // system + BP2 字节级一致（prefix_byte_hash(2) 即跨回合稳定锚点）。
        assert_eq!(
            serde_json::to_vec(&raw1[..2].to_vec()).unwrap(),
            serde_json::to_vec(&raw2[..2].to_vec()).unwrap()
        );
        assert_eq!(turn1.prefix_byte_hash(2), turn2.prefix_byte_hash(2));
        // 上一回合的 history 段在新回合里逐字节保留（仅追加不改写）。
        assert_eq!(
            serde_json::to_vec(&raw1[2..4].to_vec()).unwrap(),
            serde_json::to_vec(&raw2[2..4].to_vec()).unwrap()
        );
    }

    #[test]
    fn over_budget_prefix_or_pinned_is_config_error_not_silent_trim() {
        // §6.1 第 4 条 fail-closed；TokenBudget 字段 prefix_max/pinned_max 已勘查存在。
        let request = ContextRequest {
            ruleset_id: "rs".to_string(),
            module_id: None,
            session_id: "s".to_string(),
            turn_id: "t".to_string(),
            viewer: trpg_model::VisibilityProfile::gm(),
            token_budget: trpg_model::TokenBudget {
                prefix_max: 4,
                pinned_max: 4,
                dynamic_max: 4,
                total_max: 16,
            },
        };
        let mut big_prefix = compiled();
        big_prefix.prefix_text = "w ".repeat(64);
        let err = validate_compiled_budget(&big_prefix, &request).unwrap_err();
        assert!(err.to_string().contains("prefix"));
        let mut big_pinned = compiled();
        big_pinned.pinned_text = "w ".repeat(64);
        let err = validate_compiled_budget(&big_pinned, &request).unwrap_err();
        assert!(err.to_string().contains("pinned"));
        assert!(validate_compiled_budget(&compiled(), &request).is_ok());
    }

    /// 批5 mode 维度参数化夹具：建 global gm_skill + combat mode 包（含 mode-global 覆盖层）。
    fn combat_mode_dir(suffix: &str) -> std::path::PathBuf {
        use std::fs;
        let dir =
            std::env::temp_dir().join(format!("cache_mode_{}_{}", std::process::id(), suffix));
        fs::create_dir_all(dir.join("agent/gm_skill/global")).unwrap();
        fs::write(dir.join("agent/gm_skill/global/10_base.md"), "base skill").unwrap();
        fs::create_dir_all(dir.join("agent/gm_skill/modes/combat")).unwrap();
        fs::write(
            dir.join("agent/gm_skill/modes/combat/manifest.json"),
            r#"{"mode_id":"combat","frame_kind":"combat"}"#,
        )
        .unwrap();
        fs::write(
            dir.join("agent/gm_skill/modes/combat/global.md"),
            "[combat-mode-overlay] cinematic",
        )
        .unwrap();
        dir
    }

    /// 批5 ① mode 切换=有因失效断言：mode=None（两级）vs mode=combat（四级）gm_skill
    /// 文本不同 → system 消息字节不同 → prefix_byte_hash(1) 互不相等（预期缓存失效）。
    #[test]
    fn mode_switch_invalidates_gm_skill_prefix_bytes() {
        let dir = combat_mode_dir("inv");
        let tail = DynamicTailInput {
            user_input: "x",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let skill_none = load_gm_skill_with_mode(&dir, "rs", None).unwrap();
        let skill_combat = load_gm_skill_with_mode(&dir, "rs", Some("combat")).unwrap();
        assert_ne!(
            skill_none.as_bytes(),
            skill_combat.as_bytes(),
            "mode overlay must change gm_skill text"
        );
        let hash_none =
            TurnMessages::assemble(&compiled(), &skill_none, &[], &tail).prefix_byte_hash(1);
        let hash_combat =
            TurnMessages::assemble(&compiled(), &skill_combat, &[], &tail).prefix_byte_hash(1);
        assert_ne!(
            hash_none, hash_combat,
            "mode switch must invalidate system prefix hash (justified cache miss)"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    /// 批5 ② 同 mode 内前缀字节稳定：gm_skill 不变、history 仅追加 → prefix_byte_hash(2) 不变。
    #[test]
    fn same_mode_prefix_bytes_stable_across_turns() {
        let dir = combat_mode_dir("stable");
        let skill = load_gm_skill_with_mode(&dir, "rs", Some("combat")).unwrap();
        let tail1 = DynamicTailInput {
            user_input: "attack",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let turn1 = TurnMessages::assemble(&compiled(), &skill, &[], &tail1);
        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: "attack".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "hit".to_string(),
            },
        ];
        let tail2 = DynamicTailInput {
            user_input: "move",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: None,
        };
        let turn2 = TurnMessages::assemble(&compiled(), &skill, &history, &tail2);
        assert_eq!(
            turn1.prefix_byte_hash(2),
            turn2.prefix_byte_hash(2),
            "same mode across turns must keep prefix bytes stable"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}

/// P5.6 wiring proof: the Director brief packet enters the GM-only `[gm][BP3]` user message
/// (ON), is absent + byte-identical to baseline (OFF), and is never copied by the SYSTEM into
/// the player-visible path. The packet content is produced upstream; here we only prove the
/// assemble-level wiring (Option::filter mirror of `npc_guidance_block`).
#[cfg(test)]
mod director_packet_wiring_tests {
    use super::*;
    use serde_json::Value;
    use trpg_model::{ChatMessage, CompiledContext};

    fn compiled() -> CompiledContext {
        CompiledContext {
            prefix_text: "BP1".to_string(),
            pinned_text: "BP2".to_string(),
            dynamic_text: "BP3".to_string(),
            ..Default::default()
        }
    }

    fn base_tail<'a>(packet: Option<&'a str>) -> DynamicTailInput<'a> {
        DynamicTailInput {
            user_input: "go",
            resolved_gate_facts: &[],
            errata_blocks: &[],
            obligations_block: None,
            npc_guidance_block: None,
            director_packet_block: packet,
        }
    }

    fn last_dynamic(m: &TurnMessages) -> String {
        m.to_request_messages()
            .last()
            .and_then(|x| x.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    const PACKET: &str = "[director_packet]\nbeat_kind: reveal\n[/director_packet]";

    // ── ON: the serialized ctx.messages CONTAINS the packet signature, inside [gm][BP3]. ──
    #[test]
    fn on_packet_reaches_gm_bp3_message() {
        let history = vec![ChatMessage {
            role: "assistant".to_string(),
            content: "old".to_string(),
        }];
        let m = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(Some(PACKET)));
        let raw = serde_json::to_string(&m.to_request_messages()).unwrap();
        assert!(
            raw.contains("[director_packet]"),
            "packet must reach ctx.messages"
        );
        assert!(
            raw.contains("beat_kind: reveal"),
            "beat_kind signature must appear"
        );
        // It lands in the LAST (BP3) user message, after npc_guidance, before Player Input.
        let dynamic = last_dynamic(&m);
        assert!(dynamic.contains("[BP3: Dynamic Context]"));
        assert!(
            dynamic.find("[director_packet]").unwrap() < dynamic.find("[Player Input]").unwrap(),
            "packet must precede Player Input within the [gm] BP3 message"
        );
    }

    // ── P5 revision (Gap 3): the DIRECT packet-presence signal on the assembled messages —
    //    ON ⇒ true, OFF/empty ⇒ false. This is the correct evidence (vs the invalid bp3-hash,
    //    which hashes `compiled.dynamic` BEFORE the packet tail is appended). The live ON turn
    //    drives this exact `assemble` path, so a true here mirrors the live packet entering BP3.
    #[test]
    fn contains_director_packet_true_on_false_off() {
        let history = vec![ChatMessage {
            role: "assistant".to_string(),
            content: "old".to_string(),
        }];
        let on = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(Some(PACKET)));
        assert!(
            on.contains_director_packet(),
            "ON: assembled messages must report the packet present"
        );
        let off = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(None));
        assert!(
            !off.contains_director_packet(),
            "OFF: no packet in the assembled messages"
        );
        let empty = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(Some("  ")));
        assert!(
            !empty.contains_director_packet(),
            "empty packet is filtered out → reported absent"
        );
    }

    // ── OFF: absent AND byte-identical to the pre-change baseline (None packet). ──
    #[test]
    fn off_packet_is_byte_identical_to_baseline() {
        let history = vec![ChatMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
        }];
        let baseline = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(None));
        // An explicit empty packet must also be filtered out (Option::filter empty) → identical.
        let empty = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(Some("  ")));
        let baseline_bytes = serde_json::to_vec(&baseline.to_request_messages()).unwrap();
        let empty_bytes = serde_json::to_vec(&empty.to_request_messages()).unwrap();
        assert_eq!(
            baseline_bytes, empty_bytes,
            "OFF / empty packet must be byte-identical to baseline"
        );
        assert!(!last_dynamic(&baseline).contains("[director_packet]"));
        // Full prefix hash unchanged (mirror P1.7/P4.6 hash compare).
        assert_eq!(
            baseline.prefix_byte_hash(usize::MAX),
            empty.prefix_byte_hash(usize::MAX)
        );
    }

    // ── player-invisible: the SYSTEM never copies the packet outside the [gm] message. The
    //    only user/assistant messages are the [gm] BP2/BP3 envelopes + history; no separate
    //    player-visible message carries the packet (narrow claim: the system does not copy it,
    //    not that a model won't paraphrase). ──
    #[test]
    fn packet_is_only_in_gm_message_never_a_player_visible_one() {
        let history = vec![ChatMessage {
            role: "assistant".to_string(),
            content: "prior narration".to_string(),
        }];
        let m = TurnMessages::assemble(&compiled(), "SKILL", &history, &base_tail(Some(PACKET)));
        let msgs = m.to_request_messages();
        let mut carriers = 0;
        for msg in &msgs {
            let content = msg.get("content").and_then(Value::as_str).unwrap_or("");
            if content.contains("[director_packet]") {
                carriers += 1;
                // The sole carrier is the GM BP3 user message (a `[gm]`-wrapped envelope).
                assert!(
                    content.contains("[gm]") && content.contains("[BP3: Dynamic Context]"),
                    "packet appeared outside the [gm] BP3 message"
                );
            }
        }
        assert_eq!(
            carriers, 1,
            "exactly one (GM-only) message may carry the packet"
        );
        // The history (player-visible prior narration) is never mutated to carry it.
        assert!(!history[0].content.contains("[director_packet]"));
    }
}
