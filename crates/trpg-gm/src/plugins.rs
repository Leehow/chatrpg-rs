//! 纯 prompt 插件系统：`data/agent/plugins/*.md` 里 opt-in 的 markdown 片段，
//! 经声明式 frontmatter `applies_when` 按当前 (ruleset_id, module_id) 过滤后
//! 拼进 gm_skill。**纯 prompt——不挂任何机制/上下文过滤 hook**。
//!
//! 与 gm_skill 的关键区别：插件是**可选**的——目录缺失 fail-soft（Ok 空串），
//! 绝不像 gm_skill 那样 fail-closed。无任何匹配插件时，gm_skill 文本逐字节不变
//! （保护缓存稳定性硬回归）。
//!
//! frontmatter 语义（首个 `---` 分隔块；缺失 ⇒ 视为 `always`）：
//! ```text
//! ---
//! applies_when: <value>
//! ---
//! <prompt body>
//! ```
//! `applies_when` 取值：
//! - `always`        每个 session 都装；
//! - `module`        任何带 module_id（Some）的 session；
//! - `ruleset:<id>`  ruleset_id == id；
//! - `module:<id>`   module_id == Some(id)；
//! - `off`           永不装（开关位，留着文件但停用）。
//!
//! 所有 id 比对都是对**运行时** ruleset_id / module_id 值比对（数据驱动），id
//! 从插件 frontmatter 里解析出来——引擎里没有任何 `if ruleset=="coc"` 字面量。

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::prompts::load_gm_skill_with_mode;

/// 解析出的插件 applies_when 条件（数据驱动，id 来自 frontmatter）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppliesWhen {
    /// 每个 session 都装（含无 frontmatter / 无法识别的取值——保守按 always）。
    Always,
    /// 任何带 module 的 session（module_id 为 Some）。
    AnyModule,
    /// 指定规则集：ruleset_id == 该 id。
    Ruleset(String),
    /// 指定模组：module_id == Some(该 id)。
    Module(String),
    /// 永不装（停用开关）。
    Off,
}

impl AppliesWhen {
    /// 解析 frontmatter 里 `applies_when:` 的值字符串。无法识别 ⇒ Always（保守）。
    fn parse(raw: &str) -> Self {
        let v = raw.trim();
        if let Some(id) = v.strip_prefix("ruleset:") {
            return AppliesWhen::Ruleset(id.trim().to_string());
        }
        if let Some(id) = v.strip_prefix("module:") {
            return AppliesWhen::Module(id.trim().to_string());
        }
        match v {
            "always" => AppliesWhen::Always,
            "module" => AppliesWhen::AnyModule,
            "off" => AppliesWhen::Off,
            _ => AppliesWhen::Always,
        }
    }

    /// 当前 (ruleset_id, module_id) 是否命中本条件。
    fn matches(&self, ruleset_id: &str, module_id: Option<&str>) -> bool {
        match self {
            AppliesWhen::Always => true,
            AppliesWhen::AnyModule => module_id.is_some(),
            AppliesWhen::Ruleset(id) => ruleset_id == id,
            AppliesWhen::Module(id) => module_id == Some(id.as_str()),
            AppliesWhen::Off => false,
        }
    }
}

/// 拆分一份插件文件文本为 (applies_when, body)。
///
/// frontmatter 是文件开头第一个 `---` 分隔块：首行（去空白）须恰为 `---`，块内
/// 找 `applies_when:` 行取值，遇到下一行 `---` 收口、其后全部为 body。无开头
/// `---` ⇒ 无 frontmatter，整篇即 body、条件视为 `always`。
fn parse_plugin_frontmatter(text: &str) -> (AppliesWhen, String) {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return (AppliesWhen::Always, String::new());
    };
    if first.trim() != "---" {
        // 无 frontmatter：整篇是 body，按 always。
        return (AppliesWhen::Always, text.to_string());
    }
    let mut applies = AppliesWhen::Always;
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
                continue;
            }
            if let Some(val) = line.trim().strip_prefix("applies_when:") {
                applies = AppliesWhen::parse(val);
            }
            continue;
        }
        body_lines.push(line);
    }
    // 收口的 frontmatter 之后即 body；剥掉 body 前导空行，保持拼接整洁。
    let body = body_lines.join("\n");
    (applies, body.trim_start_matches('\n').to_string())
}

/// 装载并按当前 (ruleset_id, module_id) 过滤 `data/agent/plugins/*.md`。
///
/// 目录缺失 ⇒ Ok(空串)（插件可选，fail-soft）。命中的 body（剥去 frontmatter）
/// 按文件名字典序、以 `\n\n---\n\n` 拼接返回；无命中 ⇒ 空串。
pub fn load_plugins(data_dir: &Path, ruleset_id: &str, module_id: Option<&str>) -> Result<String> {
    let dir = data_dir.join("agent/plugins");
    if !dir.exists() {
        return Ok(String::new());
    }
    let mut files = fs::read_dir(&dir)
        .with_context(|| format!("failed reading plugins dir {}", dir.display()))?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    files.sort();
    let mut chunks = Vec::new();
    for p in files {
        let text =
            fs::read_to_string(&p).with_context(|| format!("failed reading {}", p.display()))?;
        let (applies, body) = parse_plugin_frontmatter(&text);
        if applies.matches(ruleset_id, module_id) && !body.trim().is_empty() {
            chunks.push(body);
        }
    }
    Ok(chunks.join("\n\n---\n\n"))
}

/// = `load_gm_skill_with_mode` 的四级 gm_skill 合并，再把命中的插件 body 以
/// `\n\n---\n\n` 追加。**无命中插件时与 load_gm_skill_with_mode 字节级一致**
/// （故无插件 / 非模组 session 不受影响——保护缓存稳定性硬回归）。
pub fn load_gm_skill_with_plugins(
    data_dir: &Path,
    ruleset: &str,
    mode: Option<&str>,
    module_id: Option<&str>,
) -> Result<String> {
    let mut text = load_gm_skill_with_mode(data_dir, ruleset, mode)?;
    let plugins = load_plugins(data_dir, ruleset, module_id)?;
    if !plugins.is_empty() {
        text.push_str("\n\n---\n\n");
        text.push_str(&plugins);
    }
    Ok(text)
}

#[cfg(test)]
#[path = "plugins_tests.rs"]
mod plugins_tests;
