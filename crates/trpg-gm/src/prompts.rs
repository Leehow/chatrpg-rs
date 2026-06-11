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
}

/// 回合消息容器：assemble 渲染一次、整回合复用；工具轮只在尾部 push，
/// 绝不重渲染/重排任何前缀字节（§6.1 第 2 条）。
pub struct TurnMessages { messages: Vec<Value>, assembled_len: usize }

impl TurnMessages {
    /// §6.1 布局（稳定性降序）：system(BP1+skill) → user(BP2) → history… → user(BP3+尾段)。
    pub fn assemble(compiled: &CompiledContext, gm_skill_text: &str, history: &[ChatMessage], tail: &DynamicTailInput<'_>) -> Self {
        let mut messages = Vec::new();
        messages.push(json!({"role":"system","content": format!("{}\n\n{}", compiled.prefix_text, gm_skill_text)}));
        messages.push(json!({"role":"user","content": format!("[gm]\n[BP2: Pinned Context]\n{}\n[/gm]", compiled.pinned_text)}));
        for h in history {
            messages.push(json!({"role": h.role, "content": h.content}));
        }
        let mut dynamic = format!("[gm]\n[BP3: Dynamic Context]\n{}\n[/gm]", compiled.dynamic_text);
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
        dynamic.push_str("\n\n[Player Input]\n");
        dynamic.push_str(tail.user_input);
        messages.push(json!({"role":"user","content": dynamic}));
        let assembled_len = messages.len();
        Self { messages, assembled_len }
    }

    /// 工具轮：追加 assistant tool_calls 消息（id/name/arguments 原样回放）。
    pub fn push_assistant_tool_calls(&mut self, calls: &[AggregatedToolCall]) {
        let tool_calls = calls.iter().map(|c| json!({"id": c.id, "type":"function", "function":{"name": c.name, "arguments": c.arguments}})).collect::<Vec<_>>();
        self.messages.push(json!({"role":"assistant","content": Value::Null,"tool_calls": tool_calls}));
    }

    /// 工具轮：追加一条 tool role 结果消息（content = dispatch 产物）。
    pub fn push_tool_result(&mut self, tool_call_id: &str, name: &str, content: &str) {
        self.messages.push(json!({"role":"tool","tool_call_id": tool_call_id,"name": name,"content": content}));
    }

    /// 债务门控（B6）：被拦下的叙事终态轮回填 block_text() 为 system 观察，
    /// 该轮 content 整体丢弃后循环继续（尾部追加，前缀字节不动）。
    pub fn push_system_observation(&mut self, text: &str) {
        self.messages.push(json!({"role":"system","content": text}));
    }

    /// 每轮请求体快照（Vec<Value> clone）。
    pub fn to_request_messages(&self) -> Vec<Value> { self.messages.clone() }

    /// 缓存稳定可观测：前 first_n 条消息序列化字节的稳定哈希（Task 9 回归用）。
    /// first_n 截顶到 assemble 产出段长度：工具轮尾部追加的消息永不进哈希窗口。
    pub fn prefix_byte_hash(&self, first_n: usize) -> String {
        let n = first_n.min(self.assembled_len);
        let bytes = serde_json::to_vec(&self.messages.iter().take(n).collect::<Vec<_>>()).unwrap_or_default();
        let digest = Sha256::digest(bytes);
        format!("sha256:{digest:x}")
    }
}

fn sorted_md_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() { return Ok(Vec::new()); }
    let mut files = fs::read_dir(dir)?.filter_map(|e| e.ok().map(|x| x.path())).filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md")).collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

/// gm_skill 装载：global/*.md（文件名字典序）→ <ruleset_id>/*.md 追加（两级合并）。
/// global 目录缺失 → Err（gm_skill 是 agent 路径硬依赖，fail-closed）。
pub fn load_gm_skill(data_dir: &Path, ruleset_id: &str) -> Result<String> {
    let base = data_dir.join("agent/gm_skill");
    let global = base.join("global");
    if !global.exists() { return Err(anyhow!("global gm_skill directory missing: {}", global.display())); }
    let mut chunks = Vec::new();
    for p in sorted_md_files(&global)? { chunks.push(fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?); }
    let ruleset = base.join(ruleset_id);
    for p in sorted_md_files(&ruleset)? { chunks.push(fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?); }
    Ok(chunks.join("\n\n---\n\n"))
}

/// §6.1 第 4 条 fail-closed：prefix/pinned 段超 TokenBudget ⇒ 配置错误
/// （Err 信息含 "prefix"/"pinned" 段名），绝不静默裁剪。
pub fn validate_compiled_budget(compiled: &CompiledContext, request: &ContextRequest) -> Result<()> {
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
        CompiledContext { prefix_text: "BP1".to_string(), pinned_text: "BP2".to_string(), dynamic_text: "BP3".to_string(), prefix_hash: "p".to_string(), pinned_hash: "m".to_string(), dynamic_hash: "d".to_string(), ..Default::default() }
    }

    #[test]
    fn assemble_order_is_stable() {
        let history = vec![ChatMessage { role: "assistant".to_string(), content: "old".to_string() }];
        let tail = DynamicTailInput { user_input: "go", resolved_gate_facts: &["[roll]done[/roll]".to_string()], errata_blocks: &["[gm_errata]fix[/gm_errata]".to_string()], obligations_block: Some("[obligations_carryover]debt[/obligations_carryover]") };
        let messages = TurnMessages::assemble(&compiled(), "SKILL", &history, &tail);
        let raw = messages.to_request_messages();
        assert_eq!(raw[0].get("role").and_then(Value::as_str), Some("system"));
        assert!(raw[0].get("content").and_then(Value::as_str).unwrap().contains("BP1"));
        assert!(raw[1].get("content").and_then(Value::as_str).unwrap().contains("BP2"));
        assert!(raw[2].get("content").and_then(Value::as_str).unwrap().contains("old"));
        assert!(raw[3].get("content").and_then(Value::as_str).unwrap().contains("BP3"));
        assert!(raw[3].get("content").and_then(Value::as_str).unwrap().contains("[Player Input]"));
        // B6 契约：obligations_block 排在 errata_blocks 之后、Player Input 之前。
        let dynamic = raw[3].get("content").and_then(Value::as_str).unwrap();
        assert!(dynamic.find("[gm_errata]").unwrap() < dynamic.find("[obligations_carryover]").unwrap());
        assert!(dynamic.find("[obligations_carryover]").unwrap() < dynamic.find("[Player Input]").unwrap());
    }

    #[test]
    fn empty_pinned_context_injects_get_actor_hint() {
        // BP2 空投影兜底（e2e must-fix 便宜修法）：pinned 为空串 ⇒ dynamic tail
        // 注入 [system] get_actor 提示；非空 ⇒ 绝不注入（不污染正常回合）。
        let tail = DynamicTailInput { user_input: "go", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
        let mut empty_pinned = compiled();
        empty_pinned.pinned_text = "  ".to_string();
        let messages = TurnMessages::assemble(&empty_pinned, "SKILL", &[], &tail);
        let dynamic = messages.to_request_messages().last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("").to_string();
        assert!(dynamic.contains("[system] No character projection"), "missing fail-closed hint: {dynamic}");
        assert!(dynamic.contains("get_actor"));
        let normal = TurnMessages::assemble(&compiled(), "SKILL", &[], &tail);
        let dynamic = normal.to_request_messages().last().and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("").to_string();
        assert!(!dynamic.contains("No character projection"));
    }

    #[test]
    fn push_does_not_change_prefix_hash() {
        let tail = DynamicTailInput { user_input: "go", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
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
        let p30 = text.find("Player-visible Output Contract").expect("30_output_contract content missing");
        let p40 = text.find("目录优先于自由发挥").expect("40_mechanics_catalog content missing");
        let p50 = text.find("due 必须回应").expect("50_obligation_policy content missing");
        assert!(p30 < p40, "40_ content must appear after 30_ ({p30} vs {p40})");
        assert!(p40 < p50, "50_ content must appear after 40_ ({p40} vs {p50})");
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

    fn compiled() -> CompiledContext { CompiledContext { prefix_text: "PFX".to_string(), pinned_text: "PIN".to_string(), dynamic_text: "DYN".to_string(), ..Default::default() } }

    #[test]
    fn same_inputs_assemble_same_bytes() {
        let tail = DynamicTailInput { user_input: "x", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
        let a = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        let b = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        assert_eq!(serde_json::to_vec(&a.to_request_messages()).unwrap(), serde_json::to_vec(&b.to_request_messages()).unwrap());
    }

    #[test]
    fn tool_append_changes_only_tail() {
        let tail = DynamicTailInput { user_input: "x", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
        let mut m = TurnMessages::assemble(&compiled(), "skill", &[ChatMessage { role: "assistant".to_string(), content: "old".to_string() }], &tail);
        let before = m.to_request_messages();
        m.push_assistant_tool_calls(&[AggregatedToolCall { id: "c".to_string(), name: "remember".to_string(), arguments: "{}".to_string() }]);
        let after = m.to_request_messages();
        assert_eq!(serde_json::to_vec(&before).unwrap(), serde_json::to_vec(&after[..before.len()]).unwrap());
    }

    #[test]
    fn prefix_hash_is_byte_level_hash() {
        let tail = DynamicTailInput { user_input: "x", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
        let m = TurnMessages::assemble(&compiled(), "skill", &[], &tail);
        assert_eq!(m.prefix_byte_hash(2), m.prefix_byte_hash(2));
        assert_ne!(m.prefix_byte_hash(1), m.prefix_byte_hash(2));
    }

    #[test]
    fn cross_turn_stable_segments_keep_bytes_with_growing_history() {
        // §6.1 第 3 条 / spec §9 单测 9 的回归本体：场景不变、gm_skill 不变，
        // 跨回合 history 仅追加 ⇒ system+BP2 两条消息字节不变，旧 history 段不改写。
        let turn1_history = vec![ChatMessage { role: "user".to_string(), content: "hi".to_string() }, ChatMessage { role: "assistant".to_string(), content: "scene".to_string() }];
        let tail1 = DynamicTailInput { user_input: "look", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
        let turn1 = TurnMessages::assemble(&compiled(), "skill", &turn1_history, &tail1);
        let mut turn2_history = turn1_history.clone();
        turn2_history.push(ChatMessage { role: "user".to_string(), content: "look".to_string() });
        turn2_history.push(ChatMessage { role: "assistant".to_string(), content: "you see".to_string() });
        let tail2 = DynamicTailInput { user_input: "move", resolved_gate_facts: &[], errata_blocks: &[], obligations_block: None };
        let turn2 = TurnMessages::assemble(&compiled(), "skill", &turn2_history, &tail2);
        let raw1 = turn1.to_request_messages();
        let raw2 = turn2.to_request_messages();
        // system + BP2 字节级一致（prefix_byte_hash(2) 即跨回合稳定锚点）。
        assert_eq!(serde_json::to_vec(&raw1[..2].to_vec()).unwrap(), serde_json::to_vec(&raw2[..2].to_vec()).unwrap());
        assert_eq!(turn1.prefix_byte_hash(2), turn2.prefix_byte_hash(2));
        // 上一回合的 history 段在新回合里逐字节保留（仅追加不改写）。
        assert_eq!(serde_json::to_vec(&raw1[2..4].to_vec()).unwrap(), serde_json::to_vec(&raw2[2..4].to_vec()).unwrap());
    }

    #[test]
    fn over_budget_prefix_or_pinned_is_config_error_not_silent_trim() {
        // §6.1 第 4 条 fail-closed；TokenBudget 字段 prefix_max/pinned_max 已勘查存在。
        let request = ContextRequest { ruleset_id: "rs".to_string(), module_id: None, session_id: "s".to_string(), turn_id: "t".to_string(), viewer: trpg_model::VisibilityProfile::gm(), token_budget: trpg_model::TokenBudget { prefix_max: 4, pinned_max: 4, dynamic_max: 4, total_max: 16 } };
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
}

