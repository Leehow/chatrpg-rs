-- _fix_homecoming_intent_difficulty.sql（幂等；先例对标 _fix_triangle_chaos_amount.sql 纯数据手修通道）
-- 背景（C7 收口 2026-06-11 确诊）：C2 深抽把 SceneMechanicIntent.difficulty 写成纯字符串
-- "DV13"/"DV14"（照抄 source_anchor 原文），而 scene_policy::difficulty_to_target 只认
-- 结构化 {kind:"dv"|"static"|"target_number", value:<num>}（零硬编码的设计决定，
-- 认不出 → UnknownUntilLookup → outcome.success 为 null → effect_policy fail-closed 跳过）。
-- 本修把**无歧义**的 `DVnn` 纯字符串就地升级为 {kind:"dv",value:nn}；含条件分支的字符串
-- （如 "DV12 (DV17 if no Techs at the party)"）保持原样（fail-closed 不猜）。
-- 注意：homecoming 模组重抽可能再产字符串形态（C2 深抽 difficulty 形态约束缺口，
-- 已在二期验收报告技术债清单记录）；重抽后按需重放本 SQL。
update parsed_bundles pb set content_json = jsonb_set(pb.content_json, '{module_graph,scenes}', (
  select jsonb_agg(
    case when jsonb_typeof(s->'scene_mechanics') = 'array' and jsonb_array_length(s->'scene_mechanics') > 0 then
      jsonb_set(s, '{scene_mechanics}', (
        select jsonb_agg(
          case when i->>'difficulty' ~ '^DV[0-9]+$'
               then jsonb_set(i, '{difficulty}', jsonb_build_object('kind','dv','value', (substring(i->>'difficulty' from '[0-9]+'))::int))
               else i end order by iord)
        from jsonb_array_elements(s->'scene_mechanics') with ordinality u(i, iord)))
    else s end order by ord)
  from jsonb_array_elements(pb.content_json#>'{module_graph,scenes}') with ordinality t(s, ord)
))
where bundle_kind = 'module' and content_json->>'module_id' = 'cyberpunk_red.homecoming';
