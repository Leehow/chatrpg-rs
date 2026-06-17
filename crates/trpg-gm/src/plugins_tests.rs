//! 纯函数测试：用 temp `plugins/` 夹具覆盖 applies_when 过滤、排序+`---`拼接、
//! body 剥 frontmatter；缺目录 fail-soft；以及 load_gm_skill_with_plugins 的
//! mode=None/无插件路径与 load_gm_skill 字节级一致（缓存稳定）。全程 DB-free。

use super::*;
use crate::prompts::load_gm_skill;
use std::fs;

/// 建一份隔离 temp data_dir，写入若干插件文件，返回根目录。
fn plugins_fixture(suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gm_plugins_{}_{}_{}",
        std::process::id(),
        suffix,
        uuid::Uuid::new_v4().simple()
    ));
    let plugins = dir.join("agent/plugins");
    fs::create_dir_all(&plugins).unwrap();
    // 文件名故意乱序写入，验证 load_plugins 按文件名字典序输出。
    fs::write(
        plugins.join("30_module_only.md"),
        "---\napplies_when: module\n---\nMODULE-BODY",
    )
    .unwrap();
    fs::write(
        plugins.join("10_always.md"),
        "---\napplies_when: always\n---\nALWAYS-BODY",
    )
    .unwrap();
    fs::write(
        plugins.join("20_ruleset_coc.md"),
        "---\napplies_when: ruleset:call_of_cthulhu_7e\n---\nRULESET-COC-BODY",
    )
    .unwrap();
    fs::write(
        plugins.join("40_module_specific.md"),
        "---\napplies_when: module:blood_highway\n---\nMODULE-SPECIFIC-BODY",
    )
    .unwrap();
    fs::write(
        plugins.join("50_off.md"),
        "---\napplies_when: off\n---\nOFF-BODY",
    )
    .unwrap();
    // 无 frontmatter ⇒ 视为 always；整篇即 body。
    fs::write(plugins.join("60_no_frontmatter.md"), "NO-FM-BODY").unwrap();
    dir
}

#[test]
fn parse_frontmatter_strips_block_and_reads_applies_when() {
    let (applies, body) =
        parse_plugin_frontmatter("---\napplies_when: module\n---\nhello\nworld");
    assert_eq!(applies, AppliesWhen::AnyModule);
    assert_eq!(body, "hello\nworld");
}

#[test]
fn parse_frontmatter_absent_is_always_and_full_body() {
    let (applies, body) = parse_plugin_frontmatter("just a body\nsecond line");
    assert_eq!(applies, AppliesWhen::Always);
    assert_eq!(body, "just a body\nsecond line");
}

#[test]
fn parse_applies_when_variants() {
    assert_eq!(AppliesWhen::parse("always"), AppliesWhen::Always);
    assert_eq!(AppliesWhen::parse("module"), AppliesWhen::AnyModule);
    assert_eq!(AppliesWhen::parse("off"), AppliesWhen::Off);
    assert_eq!(
        AppliesWhen::parse("ruleset:call_of_cthulhu_7e"),
        AppliesWhen::Ruleset("call_of_cthulhu_7e".to_string())
    );
    assert_eq!(
        AppliesWhen::parse("module:blood_highway"),
        AppliesWhen::Module("blood_highway".to_string())
    );
    // 未识别取值保守按 always。
    assert_eq!(AppliesWhen::parse("nonsense"), AppliesWhen::Always);
}

#[test]
fn load_plugins_module_session_includes_matching_bodies_sorted() {
    // (a) 模组 session：ruleset=call_of_cthulhu_7e, module=Some(blood_highway)。
    // 命中：always / ruleset:coc / module / module:blood_highway / 无frontmatter；
    // off 永不命中。按文件名字典序：10_always → 20_ruleset → 30_module →
    // 40_module_specific →（50_off 跳过）→ 60_no_frontmatter。
    let dir = plugins_fixture("module_session");
    let out = load_plugins(&dir, "call_of_cthulhu_7e", Some("blood_highway")).unwrap();
    assert_eq!(
        out,
        "ALWAYS-BODY\n\n---\n\nRULESET-COC-BODY\n\n---\n\nMODULE-BODY\n\n---\n\nMODULE-SPECIFIC-BODY\n\n---\n\nNO-FM-BODY"
    );
    assert!(!out.contains("OFF-BODY"), "off plugin must never load");
    // body 已剥 frontmatter（不含 applies_when 字样）。
    assert!(!out.contains("applies_when"));
    fs::remove_dir_all(dir).ok();
}

#[test]
fn load_plugins_non_module_session_excludes_module_scoped() {
    // (b) 非模组 session：module=None。命中 always / ruleset:coc / 无frontmatter；
    // module（任意模组）/ module:blood_highway / off 全不命中。
    let dir = plugins_fixture("non_module");
    let out = load_plugins(&dir, "call_of_cthulhu_7e", None).unwrap();
    assert_eq!(out, "ALWAYS-BODY\n\n---\n\nRULESET-COC-BODY\n\n---\n\nNO-FM-BODY");
    assert!(!out.contains("MODULE-BODY"));
    assert!(!out.contains("MODULE-SPECIFIC-BODY"));
    fs::remove_dir_all(dir).ok();
}

#[test]
fn load_plugins_different_ruleset_excludes_ruleset_scoped() {
    // (c) 不同规则集 + 不同模组：ruleset:coc 与 module:blood_highway 都不命中；
    // always / module / 无frontmatter 命中。
    let dir = plugins_fixture("other_ruleset");
    let out = load_plugins(&dir, "dnd5e", Some("some_other_module")).unwrap();
    assert_eq!(out, "ALWAYS-BODY\n\n---\n\nMODULE-BODY\n\n---\n\nNO-FM-BODY");
    assert!(!out.contains("RULESET-COC-BODY"));
    assert!(!out.contains("MODULE-SPECIFIC-BODY"));
    fs::remove_dir_all(dir).ok();
}

#[test]
fn load_plugins_missing_dir_is_empty_ok() {
    let dir = std::env::temp_dir().join(format!(
        "gm_plugins_absent_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    // 该 data_dir 下没有 agent/plugins ⇒ Ok 空串（fail-soft，非 fail-closed）。
    let out = load_plugins(&dir, "call_of_cthulhu_7e", Some("blood_highway")).unwrap();
    assert_eq!(out, "");
}

/// 建最小 gm_skill global 夹具（供 with_plugins 字节对比），可选追加插件。
fn gm_skill_fixture(suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gm_plugins_skill_{}_{}_{}",
        std::process::id(),
        suffix,
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(dir.join("agent/gm_skill/global")).unwrap();
    fs::write(dir.join("agent/gm_skill/global/10_base.md"), "BASE-SKILL").unwrap();
    dir
}

#[test]
fn with_plugins_no_match_is_byte_identical_to_gm_skill() {
    // mode=None 且无任何匹配插件（plugins 目录不存在）⇒ 与 load_gm_skill 字节级一致。
    let dir = gm_skill_fixture("nomatch");
    let baseline = load_gm_skill(&dir, "call_of_cthulhu_7e").unwrap();
    let with_plugins =
        load_gm_skill_with_plugins(&dir, "call_of_cthulhu_7e", None, Some("blood_highway")).unwrap();
    assert_eq!(
        baseline.as_bytes(),
        with_plugins.as_bytes(),
        "no-plugin path must be byte-identical (cache stability)"
    );
    fs::remove_dir_all(dir).ok();
}

#[test]
fn with_plugins_present_dir_but_no_filter_match_still_byte_identical() {
    // plugins 目录存在但当前 session 无命中（只有一条 off + 一条别的规则集）⇒
    // 仍与 load_gm_skill 字节级一致（空插件不追加任何分隔符）。
    let dir = gm_skill_fixture("present_no_match");
    let plugins = dir.join("agent/plugins");
    fs::create_dir_all(&plugins).unwrap();
    fs::write(plugins.join("10_off.md"), "---\napplies_when: off\n---\nX").unwrap();
    fs::write(
        plugins.join("20_other.md"),
        "---\napplies_when: ruleset:dnd5e\n---\nY",
    )
    .unwrap();
    let baseline = load_gm_skill(&dir, "call_of_cthulhu_7e").unwrap();
    // 非模组、非 dnd5e ⇒ 两条都不命中。
    let with_plugins =
        load_gm_skill_with_plugins(&dir, "call_of_cthulhu_7e", None, None).unwrap();
    assert_eq!(baseline.as_bytes(), with_plugins.as_bytes());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn with_plugins_matching_plugin_is_appended() {
    let dir = gm_skill_fixture("match");
    let plugins = dir.join("agent/plugins");
    fs::create_dir_all(&plugins).unwrap();
    fs::write(
        plugins.join("10_anti_spoiler.md"),
        "---\napplies_when: module\n---\nANTI-SPOILER-BODY",
    )
    .unwrap();
    // 模组 session ⇒ 命中 module 插件，追加在 gm_skill 之后。
    let out =
        load_gm_skill_with_plugins(&dir, "call_of_cthulhu_7e", None, Some("blood_highway")).unwrap();
    assert!(out.starts_with("BASE-SKILL"), "gm_skill must come first: {out}");
    assert!(out.contains("ANTI-SPOILER-BODY"), "matching plugin body must be appended");
    assert!(out.contains("BASE-SKILL\n\n---\n\nANTI-SPOILER-BODY"));
    // 非模组 session ⇒ 不命中 ⇒ 与基线一致。
    let no_match = load_gm_skill_with_plugins(&dir, "call_of_cthulhu_7e", None, None).unwrap();
    assert_eq!(no_match.as_bytes(), load_gm_skill(&dir, "call_of_cthulhu_7e").unwrap().as_bytes());
    fs::remove_dir_all(dir).ok();
}

#[test]
fn anti_spoiler_md_superseded_by_builtin_plugin() {
    // T2 迁移：原 data/agent/plugins/anti_spoiler.md 已删，防剧透引导改走内置
    // policy 插件 core.no_spoiler_guard（PluginHost 路径）。这里守卫**双注入回归**：
    // .md 路径（load_plugins）的真实 data 目录不再产出防剧透文本——否则模组回合会
    // 同时从 .md 与插件各注入一次。
    let data_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data");
    let with_module = load_plugins(&data_dir, "call_of_cthulhu_7e", Some("blood_highway")).unwrap();
    assert!(
        !with_module.contains("防剧透"),
        "anti_spoiler.md 已删：.md 路径不应再产防剧透文本（防双注入），现由内置插件供给"
    );
    assert!(
        !data_dir.join("agent/plugins/anti_spoiler.md").exists(),
        "anti_spoiler.md 必须已被内置插件取代（删除）"
    );
}
