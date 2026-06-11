//! 三期 §4.5 目录联动（终审返工 critical-2）：mode 激活时把 kernel
//! mechanics_catalog 经 manifest.catalog_filter 过滤，渲染为紧凑节，由
//! turn_loop 拼接进 mode 提示层（load_gm_skill_with_mode 产出的 mode 段之后
//! ——与 mode 提示同生命周期、同「有因失效」语义，BP2 级）。BP1 的
//! rule_steward 目录索引保持 mode 无感不动（缓存设计 §4.2：RarelyChanged
//! 不随 mode 失效）。本模块只有纯函数；db 装载与 fail-closed 折空（无目录/
//! 空过滤结果/db 失败 → 不注入 + tracing warn）收口在 turn_loop。
//! mode=None 路径绝不进此模块（二期字节回归照绿）。

use crate::mode::CatalogFilter;
use trpg_model::MechanicEntry;

/// 节锚点（turn_loop 注入与测试共用的稳定标识）。
pub const MODE_CATALOG_HEADER: &str = "[mode 目录子集]";

/// 条目 → 过滤三维投影（数据对数据，零代码关键词表）：
/// - kind：MechanicKind 的 serde 词汇（snake_case；Other 保留原串）；
/// - hooks：EngineHook 的 event tag 词汇（"combat_start"…）；
/// - semantic_tags：manifest 声明的 tag 词汇对条目 id/name/when_to_use/
///   description 文本的（不区分大小写）子串命中集——tag 词汇表在 manifest
///   数据侧，目录条目模型暂无结构化 tags 字段，按声明词命中是唯一不发明
///   数据的匹配方式（fail-closed：未命中不注入）。
fn entry_dimensions(entry: &MechanicEntry, filter: &CatalogFilter) -> (String, Vec<String>, Vec<String>) {
    let kind = serde_json::to_value(&entry.kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let hooks = entry
        .hooks
        .iter()
        .filter_map(|h| {
            let v = serde_json::to_value(h).ok()?;
            v.get("event").and_then(|e| e.as_str()).map(str::to_string)
        })
        .collect();
    let text = format!("{} {} {} {}", entry.id, entry.name, entry.when_to_use, entry.description).to_lowercase();
    let tags = filter
        .semantic_tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && text.contains(&t.to_lowercase()))
        .map(str::to_string)
        .collect();
    (kind, hooks, tags)
}

/// 过滤 + 渲染：标题行（写明这是当前姿态的机制子集）+ 每条
/// `id | name | when_to_use` 一行（空白归一保持单行不变量）。目录为空 /
/// 无条目命中 → None（不写空节，fail-closed）；三维全空过滤器 = 不过滤
/// （CatalogFilter::matches 既定语义，全量注入）。确定性：同输入同字节。
pub fn mode_catalog_section(mode_id: &str, filter: &CatalogFilter, catalog: &[MechanicEntry]) -> Option<String> {
    let lines: Vec<String> = catalog
        .iter()
        .filter(|e| {
            let (kind, hooks, tags) = entry_dimensions(e, filter);
            filter.matches(&kind, &hooks, &tags)
        })
        .map(|e| format!("{} | {} | {}", one_line(&e.id), one_line(&e.name), one_line(&e.when_to_use)))
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "{MODE_CATALOG_HEADER} 当前姿态（{mode_id}）可用的机制子集——按 mode manifest 的 catalog_filter 自本规则集 mechanics_catalog 过滤；每行 `id | name | when_to_use`，按 id 用 lookup_mechanic 取全文：\n{}",
        lines.join("\n")
    ))
}

/// 空白归一单行（条目绝不破坏每条一行的不变量；与 BP1 索引同款约定）。
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use trpg_model::{EngineHook, MechanicKind};

    fn entry(id: &str, kind: MechanicKind, hooks: Vec<EngineHook>, when_to_use: &str) -> MechanicEntry {
        MechanicEntry {
            id: id.to_string(),
            name: format!("{id} 名称"),
            kind,
            when_to_use: when_to_use.to_string(),
            hooks,
            ..Default::default()
        }
    }

    fn combat_filter() -> CatalogFilter {
        CatalogFilter {
            kinds: vec!["reaction".to_string()],
            hooks: vec!["combat_start".to_string()],
            semantic_tags: vec!["initiative".to_string()],
        }
    }

    /// 命中：kind / hook / semantic_tag 三维各一条全被选中；行格式
    /// `id | name | when_to_use`；标题行声明当前姿态的机制子集。
    #[test]
    fn filter_hits_render_one_line_per_entry_with_header() {
        let catalog = vec![
            entry("shield_parry", MechanicKind::Reaction, vec![], "when attacked in melee"),
            entry("battle_alarm", MechanicKind::Other("alarm".into()), vec![EngineHook::CombatStart], "when combat begins"),
            entry("turn_order", MechanicKind::Other("order".into()), vec![], "roll Initiative order at the start"),
        ];
        let text = mode_catalog_section("combat", &combat_filter(), &catalog).expect("hits must render a section");
        assert!(text.starts_with(MODE_CATALOG_HEADER), "header line must lead the section: {text}");
        assert!(text.contains("combat"), "header must state the current posture: {text}");
        assert!(text.contains("shield_parry | shield_parry 名称 | when attacked in melee"), "kind hit line missing: {text}");
        assert!(text.contains("battle_alarm | battle_alarm 名称 | when combat begins"), "hook hit line missing: {text}");
        assert!(text.contains("turn_order | turn_order 名称 | roll Initiative order at the start"), "semantic-tag substring hit (initiative) missing: {text}");
    }

    /// 不命中：三维全不沾边的条目绝不出现；目录全不命中 / 空目录 → None。
    #[test]
    fn filter_misses_are_dropped_and_all_miss_renders_nothing() {
        let catalog = vec![
            entry("library_use", MechanicKind::SkillCheck, vec![], "research in a library"),
            entry("shield_parry", MechanicKind::Reaction, vec![], "when attacked"),
        ];
        let text = mode_catalog_section("combat", &combat_filter(), &catalog).unwrap();
        assert!(!text.contains("library_use"), "miss entry must be filtered out: {text}");
        let all_miss = vec![entry("library_use", MechanicKind::SkillCheck, vec![], "research in a library")];
        assert!(mode_catalog_section("combat", &combat_filter(), &all_miss).is_none(), "all-miss must render no section");
        assert!(mode_catalog_section("combat", &combat_filter(), &[]).is_none(), "empty catalog must render no section");
    }

    /// 三维全空过滤器 = 不过滤（manifest 没声明维度 ⇒ 全量注入，
    /// CatalogFilter::matches 既定语义）。
    #[test]
    fn empty_filter_means_no_filtering() {
        let catalog = vec![
            entry("library_use", MechanicKind::SkillCheck, vec![], "research"),
            entry("shield_parry", MechanicKind::Reaction, vec![], "when attacked"),
        ];
        let text = mode_catalog_section("downtime", &CatalogFilter::default(), &catalog).expect("empty filter injects all");
        assert!(text.contains("library_use") && text.contains("shield_parry"), "all entries must be injected: {text}");
    }
}
