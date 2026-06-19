//! R2 收口不变量：获取类直连/env fallback 必须从 kernel 物理移除，
//! 仅经 NeedBus→resolver 触达；投影类与 memory（本期非目标）保持不动。
//! 这是源码文本不变量测试（不需 DB），守护回归不偷偷复活旧直连。
const LIB_SRC: &str = include_str!("../src/lib.rs");

/// prepare_turn_context 函数体切片（到下一 pub async fn 截断），
/// 把断言锁定在 kernel 装配段，不误伤方法定义体或其它函数。
fn prepare_turn_context_body() -> &'static str {
    let start = LIB_SRC
        .find("pub async fn prepare_turn_context")
        .expect("prepare_turn_context must exist");
    let rest = &LIB_SRC[start..];
    // 函数体后第一个顶层 `\n    pub async fn ` 作为结束锚
    let end_rel = rest[1..]
        .find("\n    pub async fn ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    &rest[..end_rel]
}

#[test]
fn getter_direct_calls_removed_from_kernel() {
    let body = prepare_turn_context_body();
    for forbidden in [
        "self.auto_search_blocks_for_turn(",
        "self.learned_packet_blocks_for_turn(",
        "self.module_scene_blocks_for_turn(",
        "self.materialization_blocks_for_turn(",
        "self.actor_parameter_blocks_for_turn(",
    ] {
        assert!(
            !body.contains(forbidden),
            "getter-class direct call `{forbidden}` must be removed from prepare_turn_context (only via NeedBus)",
        );
    }
}

#[test]
fn projection_calls_and_memory_preserved_in_kernel() {
    let body = prepare_turn_context_body();
    // 断言"调用点保留"用**带前导点的方法调用** token（`.method(`）而非 `self.method(`：
    // rustfmt 会把长链 `match self\n    .memory_blocks_for_turn(...)` 的 `self` 与 `.method(`
    // 拆到两行,使 `self.method(` 连续子串不再出现(纯格式,调用仍在)。前导点唯一标识调用点
    // (方法**定义** `async fn method(` 无前导点,且 body 切片已截到下一 pub async fn 之前),
    // 故对格式重排稳健 —— 修预存的脆性 grep 断言。
    for required in [
        ".memory_blocks_for_turn(", // 本期非目标，必须保留
        ".state_frame_blocks_for_turn(",
        ".world_time_blocks_for_turn(",
        ".object_blocks_for_turn(",
        ".ability_blocks_for_turn(",
        ".rule_binding_blocks_for_turn(",
        ".player_value_referee_blocks_for_turn(",
        ".referee_combat_blocks_for_turn(",
        ".contest_blocks_for_turn(",
        // BP1 active-kernel projection (非 query-driven 获取，原样保留)
        ".rule_steward_prefix_blocks_for_turn(",
    ] {
        assert!(
            body.contains(required),
            "projection/out-of-scope call `{required}` must stay in prepare_turn_context (unchanged)",
        );
    }
}

#[test]
fn need_bus_is_the_getter_path() {
    let body = prepare_turn_context_body();
    assert!(
        body.contains("resolve_all"),
        "kernel must drive getters via bus.resolve_all()"
    );
}

#[test]
fn need_bus_env_fallback_removed() {
    for forbidden in [
        "TRPG_NEED_BUS_RULE",
        "TRPG_NEED_BUS_SCENE",
        "TRPG_NEED_BUS_MATERIAL",
        "TRPG_NEED_BUS_PARAM",
        "TRPG_NEED_BUS_ENTITY",
    ] {
        assert!(
            !LIB_SRC.contains(forbidden),
            "env fallback `{forbidden}` must be removed at closure stage",
        );
    }
}

#[test]
fn search_async_not_called_directly_in_kernel() {
    // SearchService 仅经 RuleNeedResolver→steward.assist 触达；
    // prepare_turn_context 本体不得直连 search_async。
    let body = prepare_turn_context_body();
    assert!(
        !body.contains("search_async"),
        "SearchService must not be reached directly from prepare_turn_context",
    );
}

#[test]
fn no_need_bus_env_keyword_remains_in_lib() {
    // 收口后 lib.rs 全文不得再含 need-bus env fallback 关键字（gate fn 一并删除）。
    assert!(
        !LIB_SRC.contains("TRPG_NEED_BUS"),
        "no need-bus env fallback keyword may remain anywhere in lib.rs"
    );
}
