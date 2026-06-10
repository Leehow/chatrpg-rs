# Section C（续）—— e2e 黄金链任务正文（C5–C7）

> 接 `section_C.md`（C1–C4）；隶属计划骨架 `docs/superpowers/plans/2026-06-10-rule-aware-gm.md`，权威 spec `docs/superpowers/specs/2026-06-10-rule-aware-gm-design.md`（§8 共 14 条验收 + 附录 A 137 条审计基线）。
> 本文件三任务全部是 **e2e 验收任务**：原则上零代码改动——发现 bug 回所属实现任务（A/B/C1-C4）修复后重跑，**不顺手打补丁**；唯一例外 C7① 允许新建一个 `#[ignore]` 集成测试文件。
> e2e 环境总备忘（端口 swap / TRPG_DATA_DIR / docker psql / play --agent 管道 / 库内模组清单）见 `section_C.md` 头部；本文件命令可直接照抄，全部在 workspace 根执行。所有 `// grounded:` 锚点已对照真实源码/迁移核实（2026-06-10 工作副本）。

**共用准备（每个 shell 会话先粘贴这一段）**：
```bash
cd /Users/haoli/leehow/code/chatrpgv2/chatrpg-rs-v1.20-formula
export DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg   # .env 写的是 :54346（赛博库）——swap 坑，显式 export 必赢
export TRPG_DATA_DIR="$PWD/data"
PSQL() { docker exec chatrpg-postgres-rulesets psql -U chatrpg -d chatrpg -tAc "$1"; }   # 宿主机无 psql，一律 docker exec
cargo build -p trpg-cli 2>&1 | tail -2     # 预热编译，防首回合计时掺编译时间
# 行级时间戳跑批：stdout 每行打相对秒（TTFT/回合时长都从这份 log 读）；macOS 无 timeout(1)，超时交给执行工具参数
run_play() {  # $1=ruleset_id  $2=module_id  $3=输入文件  $4=日志前缀
  RUST_LOG=gm_cache=info,info cargo run -q -p trpg-cli -- play --ruleset "$1" --module "$2" --agent \
    < "$3" 2>"$4.stderr.log" | python3 -u -c '
import sys,time; t0=time.time()
for line in sys.stdin: print(f"[{time.time()-t0:7.2f}s] {line}",end="",flush=True)' | tee "$4.stdout.log"
}
sid_of() { grep -m1 'agent session:' "$1.stdout.log" | sed 's/.*agent session: //' | tr -d '[:space:]'; }
```
- `// grounded:` `play --agent` 走 GmLoop（main.rs L530 `Commands::Play{ruleset,module,agent}`）；session_id 打印格式 `agent session: <id>`（agent_play.rs L33）；**`trpg turn` 走旧路径 run_turn_once 不经 GmLoop，e2e 一律用 play --agent**。
- `// grounded:` gm_cache tracing 每工具轮一行，字段 prefix_hash/pinned_hash/request_prefix_hash/tool_round（turn_loop.rs L70-80）；回合收尾 context_hashes 持久化进 `turns` 表（turn_loop.rs L169 + trpg-db save_turn L986：`turns(session_id,turn_id,user_input,assistant_output,context_hashes,...)`）。

**计时与 TTFT 记录方式（三链统一，记进各任务执行记录）**：
- TTFT：`*.stdout.log` 中「输入回显行（上一个 `[chatrpg:agent]>` 提示行之后）」到「本回合第一行叙事字节」的相对秒差；回合总时长：到「下一个提示符行」的差。
- 工具轮数：`grep 'gm agent prompt cache anchors' *.stderr.log` 按 turn_id 分组计数（tool_round 字段）——债务门控回填会体现为额外轮。
- 对照基线：B8 收口冒烟记录的无债回合时长/轮数（spec §9 风险 4 的实测回答）。记录表模板：`| turn | tool_rounds | TTFT(s) | total(s) | 备注（due/waive/effect_policy 发生否）|`。

**chatrpg-product-evaluator 对照说明（三链统一）**：每条链 SQL 断言全过之后，可调 `chatrpg-product-evaluator` skill 以产品视角复评该把 transcript（GM 是否把机制叙事化得自然：SAN 链恐怖节奏、band 分档张力、池子高位的基调变化）。其结论**只作叙事质量的辅助证据**（验收 10"叙事反映池状态"、12③"叙事分档"两处判定参考）；机械断言一律以 SQL/账本为准——evaluator 不通过不阻塞机械验收，但结论必须记进执行记录与 C7 对账表备注。

---

## C5. e2e 黄金链一：CoC SAN→疯狂全链 + 跳坑感知 + 成功度三件套（验收 7/8/12）

### 范围与前置
- 真 :54347 DB + 真 LLM（relay 既有配置，loop 模型不动）+ 规则集 `call_of_cthulhu_7e` / 模组 `call_of_cthulhu_7e.document`（血色公路，已在库）。
- 前置硬依赖：**A7**（CoC 目录已编译入活动 kernel）、**B8**（Slice B 收口冒烟过）、**C1–C4 已装**。任一预检不过 → 回对应任务修，本任务不开跑。
- 零代码任务；产出 = 执行记录段落（计时表 + SQL 证据原文 + 把数与 log 文件名），供 C7 对账表逐条引用。

### 预检（照抄；任一为空/不符 → 停，回 A4/A7）
```bash
# ① 目录非空（A7 产物）
PSQL "select jsonb_array_length(content_json->'mechanics_catalog') from rule_kernels where ruleset_id='call_of_cthulhu_7e' and active=true"
# ② sanity 轨阈值带 followup_procedure_id（A4 写回）；同时记下轨 id、loss_in_one_go 阈值 N、tested_parameter 绑定
PSQL "select t->>'id', jsonb_pretty(t->'thresholds') from rule_kernels, jsonb_array_elements(content_json->'resource_tracks') t where ruleset_id='call_of_cthulhu_7e' and active=true and t->>'id' ilike '%san%'"
# ③ success_bands 含 critical/failure 且 semantics 非空（验收 12① 复检；主验已在 A7，缺 → 回 A4）
PSQL "select b->>'id', coalesce(b->>'semantics','<NULL>') from rule_kernels, jsonb_array_elements(content_json->'dice_core'->'success_bands') b where ruleset_id='call_of_cthulhu_7e' and active=true"
# ④ 验收 8 的"答案钥匙"：拉目录全量 → **语义判定**跳跃条目（禁 ILIKE 关键词筛——人或审计 subagent 读 when_to_use 语义选取），
#    记 JUMP_ID 与 JUMP_PARAM 备用（技能名由此从目录解析得出，绝不写死进断言——护栏 §3.5）
PSQL "select e->>'id', e->>'tested_parameter', e->>'when_to_use' from rule_kernels, jsonb_array_elements(content_json->'mechanics_catalog') e where ruleset_id='call_of_cthulhu_7e' and active=true"
```

### ① SAN→疯狂全链（验收 7）
```bash
cat > /tmp/c5_san.txt <<'EOF'
我推开太平间冷柜，凑近看清那具不该存在的尸体的脸
我深呼吸，强迫自己继续检查尸体上的伤痕
/quit
EOF
run_play call_of_cthulhu_7e call_of_cthulhu_7e.document /tmp/c5_san.txt c5_san_run1
SID=$(sid_of c5_san_run1)
```
- 输入只给恐怖刺激，**绝不出现"检定/SAN/理智/roll"字样**——SAN check 必须由目录 when_to_use 语义驱动放出（验收 7"非玩家明示"）。第二行输入是 agent 处理 due 的窗口（due 也可能当回合就被债务门控回填逼着处理——两种时序都合法）。
- **非确定性纪律**：全链要求「SAN check 失败且单次损失 ≥ 预检②的阈值 N」。真骰随机 → 最多重跑 5 把（c5_san_run2…，每把新 session），任一把命中即取该把做终验；5 把全未命中 → 记录各把实际 loss 值，验收 7 的 due 半边以 B4 单测 + 下方 1)2) 半链 SQL 为证据，C7 对账表标「e2e 全链未观测（概率未命中）」。**禁止**直改 DB 凑数——绕开 watcher 结算单点 = 假证据。

终验 SQL（命中把跑全部；未命中把跑 1)2) 半链）：
```bash
# 1) SAN check 契约存在，tested_parameter 与预检②记下的 sanity 轨绑定参数比对（变量对账，不写死字面）
PSQL "select check_id, contract_json->>'tested_parameter', status, created_at from check_contracts where session_id='$SID' order by created_at"
# 2) SAN current 真实下降 ≥N（current 单一存储 generic_parameter_states）
PSQL "select target_id, parameter_path, value_json, world_tick, updated_at from generic_parameter_states where session_id='$SID' order by updated_at"
# 3) watcher due（B4 / 迁移 0027）：source='threshold'、evidence 含 before/after/delta、followup_procedure_id 指向预检②的疯狂条目
PSQL "select due_id, source, source_track, followup_procedure_id, evidence, status, waive_reason, waive_scope from mechanic_dues where session_id='$SID' order by created_at"
# 4) due 去向二选一（验收 7 收口）：
#    a. 疯狂检定：后续契约 advice_refs 含 mechanic:{followup_procedure_id}（或 tested_parameter 与该条目绑定参数一致）且 due status='resolved'
PSQL "select check_id, contract_json->'advice_refs', contract_json->>'tested_parameter' from check_contracts where session_id='$SID' order by created_at"
#    b. waive：due status='waived' 且 waive_reason 非空 + 勘误记忆 gm_waive 落账（B6 副作用三连）
PSQL "select summary, tags from memory_events where session_id='$SID' and tags @> array['gm_waive']"
```

### ② 跳坑感知（验收 8）
```bash
cat > /tmp/c5_jump.txt <<'EOF'
走廊地板塌出一道两米宽的裂隙，我后退几步助跑跳过去
/quit
EOF
run_play call_of_cthulhu_7e call_of_cthulhu_7e.document /tmp/c5_jump.txt c5_jump_run1
SID2=$(sid_of c5_jump_run1)
PSQL "select contract_json->>'tested_parameter', contract_json->'advice_refs' from check_contracts where session_id='$SID2' order by created_at"
```
- 断言（护栏 §3.5——技能名来自目录非测试硬编码）：契约存在（玩家没提检定，**契约存在本身就是"感知"的证据**）且 `tested_parameter == JUMP_PARAM`（预检④语义选出）。advice_refs 含 `mechanic:$JUMP_ID` 为加分项非必须——目录是知识不是枷锁，agent 不带 mechanic_id 但参数一致同样通过。
- 失败分型（指回所属任务，不在本任务修）：无契约 = 背景板回潮 → 查 BP1 索引是否注入（stderr gm_cache 行存在性 + B1 单测）与 gm_skill 40 号准则装载（B8）；契约参数错绑 → 查 A7 目录该条目 tested_parameter 与 A3 finalize 校验。

### ③ 成功度三件套（验收 12②③；12① 主验在 A7、预检③复检）
```bash
# ② 机械触发定性：CoC kernel 是否真解析出 band 触发 / max_of（A4 写回 + A5 读取点）
PSQL "select t->>'id', o->'trigger', o->>'amount' from rule_kernels, jsonb_array_elements(content_json->'resource_tracks') t, jsonb_array_elements(t->'on_outcome') o where ruleset_id='call_of_cthulhu_7e' and active=true and (o->'trigger'->>'kind'='band' or coalesce(o->>'amount','') like 'max_of:%')"
```
- **解析出** → 机械路径证据二选一：(a) 本任务各把里真碰到 fumble（`PSQL "select check_id, outcome from check_results where session_id in ('$SID','$SID2')"` 看 band 字段）→ 对照 generic_parameter_states 前后值验损失 = 骰式最大值；(b) fumble ~5% **不强等**——以 A5 单测（max_of 纯函数 + band trigger 结算路径）+ 上面 SQL 的真 kernel 数据为证据，记录「e2e 未观测 fumble，机械路径由 A5 单测 + kernel 真数据保障」。
- **未解析出** → 记**目录缺口**：查 `PSQL "select validation_report from rule_kernels where ruleset_id='call_of_cthulhu_7e' and active=true"` 应有对应记录；C7 对账表 12② 标「缺口可观测」——**不许静默通过**（spec §8.12 原文要求）。
- ③ 语义投影双面取证：
  - 落库面：`check_results.outcome` 的 band/success_tier 字段（band 真实发生的证据）；工具结果 JSON 的 band_semantics 行**不落库**，其渲染正确性由 B3 单测保障——e2e 不重复验函数。
  - 叙事面：对照 `*.stdout.log` transcript——extreme 把与 regular 把的叙事分档明显（贯穿/卓越效果 vs 普通成功），evaluator 对照辅助判定；两个不同 band 的把数不足时多跑 1-2 把凑齐对照面（每把都查 check_results 记 band）。

### 验证（任务收口自查）
- 验收 7：四步 SQL 证据齐（或半链 + 概率未命中记录）；验收 8：参数对账通过；验收 12②③：机械定性结论 + 双面取证记录在案。
- 计时表（头部模板）必交：每把 TTFT / 回合时长 / 工具轮数，与 B8 基线对比，债务处理多出的轮数/秒数单独标注。
- 所有 SQL 输出原文 + log 文件名整理成执行记录段落（C7 对账表的 7/8/12 行直接引用）。

---

## C6. e2e 黄金链二：Homecoming 场景意图 + Triangle chaos 累积感知（验收 9/10）

### 范围与前置
- 验收 9（Homecoming）只需 **C1–C4**（effect_policy 执行复用一期原语，不依赖 A/B）；验收 10（Triangle）还需 **A7**（Triangle 目录含 chaos 条目）+ **B3**（track 语义投影 + owner_kind 通用）+ **B4**（落账单点 watcher——chaos 是 scene 级实证）。
- 模组现状（section_C.md 备忘⑥）：Triangle `triangle_agency.the_vault` 已在库；**Homecoming 库里无 bundle**（只有 `data/modules/CPR One Shot - Homecoming ver3.0 (Colored).pdf` + `data/markdown/modules/cpr_one_shot_homecoming_ver3_0_colored.md`），先 parse。
- 零代码任务；ruleset_id / module_id / 对象 id / chaos 轨 id **全部实查 SQL 得出，不在命令里硬编码**（下文 `<占位>` 执行时替换）。

### ① Homecoming 场景意图（验收 9：effect_policy 强制执行非叙事声明）

**入库与重抽（照抄）**：
```bash
# 1) 实查 CPR 规则集 id（命名以库为准）
PSQL "select ruleset_id from rule_kernels where active=true"
# 2) Homecoming 入库：parse-all 扫 data/ 全量，规则书/已入库模组 cached 跳过，净增量只有 Homecoming
cargo run -q -p trpg-cli -- parse-all 2>&1 | tail -20
# 3) 实查模组 id（含 Homecoming 的新行；下文记为 <HC_MID>）
PSQL "select content_json->>'module_id' from parsed_bundles where bundle_kind='module'"
#    若此前抽过旧版（无 scene_mechanics 的深抽）→ 删 bundle 强制重抽（既有 e2e 流程：规则书 cached 只重抽模组）：
#    PSQL "delete from parsed_bundles where bundle_kind='module' and content_json->>'module_id'='<HC_MID>'" && cargo run -q -p trpg-cli -- parse-all
# 4) 预检：图谱存在带 effect_policy 的切线缆类 intent（C2 深抽产物）。
#    懒抽架构下该场景 parse 时可能仍 SkeletonOnly（intents 只在深抽时产出）——本查询为空且场景未深抽 ≠ 失败，
#    play 导航到场深抽后再查（load_module_graph 读当前 bundle 单一事实源，到场深抽产物运行时可见——grounded: trpg-db L450-460）。
PSQL "select n->>'node_id', n->>'extraction_status', jsonb_pretty(n->'scene_mechanics') from parsed_bundles, jsonb_array_elements(content_json->'module_graph'->'scenes') n where bundle_kind='module' and content_json->>'module_id'='<HC_MID>' and jsonb_array_length(coalesce(n->'scene_mechanics','[]'::jsonb))>0"
#    记下目标 INTENT_ID、其 tested_parameter/difficulty、effect_policy 内的 object_id（如 athena_cable 类）与各分支条目数
```
- 深抽后仍无任何 intent（含目标场景已 DeepExtracted）→ 回 C2（DEEP_SYS 指引/解析过滤）排查；模组原文真没写明检定则换含明确 DV 检定的场景做锚（以模组 markdown 原文为准），**不许放宽 source_anchor 收口来凑条目**。

**play 到场并触发**：
```bash
cat > /tmp/c6_cable.txt <<'EOF'
我们沿着任务简报的路线，直接摸到执法者据点后侧的设备井
我抄起断线钳，全力剪断那根主电缆
让我看看周围有什么动静
/quit
EOF
run_play <CPR_RULESET_ID> <HC_MID> /tmp/c6_cable.txt c6_cable_run1
SID=$(sid_of c6_cable_run1)
```
- 输入按当把实际开场调整（线性模组 1-2 回合可导航到场；scene_navigator 语义切换 + 到场深抽自动发生）。第二行行动**语义对应 intent description，不提检定/DV/技能名**；第三行留给倒计时/后果的观察回合。
- 重跑纪律同 C5（≤5 把）；**失败把不白跑**：on_failure 分支真实执行同样是验收 9 证据（成功/失败各验各的分支集合）。

**终验 SQL**：
```bash
# 1) 契约带结构化引用（C4 写入 advice_refs）：scene_mechanic:{INTENT_ID}
PSQL "select check_id, contract_json->'advice_refs', contract_json->>'tested_parameter', status from check_contracts where session_id='$SID' order by created_at"
# 2) C4 证据单点：world_event kind=EffectApplied、event_json.source='scene.policy'（含 success 与 patches 摘要）
PSQL "select event_kind, jsonb_pretty(event_json) from world_events where session_id='$SID' and event_json->>'source'='scene.policy'"
# 3) 对象态真实落库（object_id 用预检 4) 从 effect_policy 读出的值，不硬编码）：mechanical_state 含 patch 内容
PSQL "select object_id, jsonb_pretty(mechanical_state), active from object_instances where session_id='$SID'"
# 4) 倒计时真实落库（StartCountdown → scheduled_events pending，payload 含 intent_id）
PSQL "select scheduled_event_id, due_tick, event_kind, payload_json, status from scheduled_events where session_id='$SID' order by created_at"
# 5) ModifyTrack 类条目（若该 intent 有）：generic_parameter_states 对应 path 变化
PSQL "select target_kind, target_id, parameter_path, value_json from generic_parameter_states where session_id='$SID'"
```
- **判定**：实际走到的分支（成功→on_success / 失败→on_failure）的 EffectPatchIntent **逐条**对得上落库证据——几条 intent 就几条证据行（SetObjectState→3)、StartCountdown→4)、ModifyTrack→5)、CreateFact/Other→2) 的 patches 摘要含 unexecutable_intent）。**叙事里声称的效果在库里没有对应行 = 验收 9 失败**（效果留给了叙事）。难度继承核对：契约 target 与 intent.difficulty 一致（contract_json 里看 dv/static 值）。

### ② Triangle chaos 累积感知（验收 10）

**预检**：
```bash
# 1) Triangle 规则集 id 实查（同上 rule_kernels）；记 <TRI_RID>
# 2) 目录含 chaos 花费/异常体条目（A7 产物；kind=spend/subsystem_procedure）——语义判定哪几条是，记 id
PSQL "select e->>'id', e->>'kind', e->>'when_to_use' from rule_kernels, jsonb_array_elements(content_json->'mechanics_catalog') e where ruleset_id='<TRI_RID>' and active=true"
# 3) chaos 轨真实 id 与 owner_kind（应为 scene 级——B3/B4 的 owner_kind 通用通路实证面）；记 <CHAOS_ID>
PSQL "select t->>'id', t->>'owner_kind', t->'thresholds' is not null, t->>'zero_means' from rule_kernels, jsonb_array_elements(content_json->'resource_tracks') t where ruleset_id='<TRI_RID>' and active=true"
```

**play ≥3 回合**（每回合行动自然引发检定——Triangle 每掷 6d4 非 3 面进池，池子必然递增）：
```bash
cat > /tmp/c6_chaos.txt <<'EOF'
我用能力扫描金库大厅里的异常痕迹
我撬开通风管道钻进去，朝核心区摸过去
我直接对守卫使出我的异常能力
事情闹这么大了，我环顾四周看看现实出了什么问题
/quit
EOF
run_play <TRI_RID> triangle_agency.the_vault /tmp/c6_chaos.txt c6_chaos_run1
SID2=$(sid_of c6_chaos_run1)
```

**终验**：
```bash
# 1) chaos 池递增可查（scene 级通路）：target_kind 与 kernel owner_kind 一致、value 随 updated_at 单调不减且 > 初值
PSQL "select target_kind, target_id, parameter_path, value_json, updated_at from generic_parameter_states where session_id='$SID2' and parameter_path like '%<CHAOS_ID>%' order by updated_at"
# 2) 检定真实发生 ≥3 次（池子来源）：
PSQL "select count(*) from check_results r join check_contracts c on r.check_id=c.check_id where c.session_id='$SID2'"
# 3) 池子高位的 due/账面反应（若 kernel chaos 轨有 thresholds → B4 应产 scene 级 due）：
PSQL "select due_id, source_track, owner_kind, owner_id, evidence, status from mechanic_dues where session_id='$SID2' order by created_at"
# 4) 叙事/行动反映池状态（验收 10"累计系统真的被记得并加强影响"）——transcript 落库面：
PSQL "select turn_id, left(assistant_output, 300) from turns where session_id='$SID2' order by created_at"
```
- **判定三层**：
  1. **结算落账**：1) 递增 + 2) 检定计数 ≥3（机械事实，硬断言）；
  2. **语义可见**：BP3 语义状态行（B3 `track_semantic_line`：当前值所处 thresholds 区间 consequence / zero_means）——BP3 全文不持久化，函数面由 B3 单测保障；e2e 行为面证据 = 后段回合叙事/工具调用体现池状态知识（4) 的 transcript + evaluator 对照判定）。kernel 无 thresholds/zero_means 可渲染 → 裸数值是正确的 fail-closed 行为，记录之，不算失败；
  3. **联动可触发**：池子高位时 agent 行动或叙事反映（GM 替异常体花池/现实扭曲基调/due 处理）——凭 3) 账面 + 4) 叙事证据综合判定，evaluator 结论记备注。
- 背景板防治三件套（护栏 §3.5.6）齐活才算验收 10 通过；缺哪件标哪件，指回所属任务（落账→B4、可见→B3、联动→B5/B6）。

### 验证（任务收口自查）
- 验收 9：分支集合逐条对账表（intent 条目 ↔ SQL 证据行）+ 成功/失败至少各观测一种分支（把数允许内）；验收 10：三件套各自证据齐。
- 计时表照 C5 模板记录（Homecoming 含到场深抽的把，深抽耗时单独标注，不混进回合时长基线）。
- SQL 输出原文 + log 文件名整理成执行记录段落（C7 对账表 9/10 行直接引用）。

---

## C7. e2e 黄金链三：追溯债务闭环 + 缓存回归 + spec 14 条验收逐条对账收口（验收 11/13 + 总对账）

### 范围与前置
- 前置：**B6/B7**（ObligationLedger/RetroactiveEffectDebt/referenced_ledger_ids）——验收 11；**B1 + C3**（BP1 索引/BP2 intents 块）——验收 13；**全部任务 A1–C6 完成**——总对账才有意义。本任务是整个二期的**最后收口闸门**。
- 唯一允许的新文件：`crates/trpg-gm/tests/retro_debt_e2e.rs`（`#[ignore]` 集成测试，真 DB + MockLlm 脚本——验收 11 的"叙事声称伤害但未调工具"靠真 LLM 不可复现，必须脚本可控；spec §8.11 原文即"MockLlm 脚本"）。
- 产出：总验收报告（中文）落 `docs/规则感知GM二期验收报告_2026-06-10.md`，核心是下方对账表执行时逐条填实。

### ① 追溯债务闭环（验收 11，MockLlm 脚本可控 + 真库状态变化）

**Create** `crates/trpg-gm/tests/retro_debt_e2e.rs`（`#[ignore]`，运行需 `DATABASE_URL` 指 :54347；对标 turn_loop_tests.rs 的 MockLlm/GmLoop 装配样板，但 engine 用真 Db——`Db::connect(env DATABASE_URL)` + `migrate()`，session 经 `engine.start_session` 真 bootstrap，绝不自造 session_id）：
```rust
// 测试是规格——两回合脚本：
// 回合 1：MockLlm 不调任何工具，直接叙事「子弹擦过你的肩膀，你掉了 3 点生命」
//   断言：verify_after_stream 后 GmLoop 持久字段含 1 条 RetroactiveEffectDebt
//   （finding_detail 含 InventedEffect 的 detail；grounded: B6 契约 §5 + B7 的
//   referenced_ledger_ids 传账本 id 全集——本回合账本为空 → 子串扫描回退路径，
//   finding detail 应带 "fallback:substring_scan" 前缀，一并断言=B7 可观测技术债半边）。
// 回合 2：MockLlm 脚本三步：
//   step1 收到 BP3/障碍观察（断言 messages 含 obligations_block 或债务回填文本，
//          block_text 里能看到 debt_id 与 finding 摘要）；
//   step2 调 apply_effect（HP -3，target=测试角色）补落账 → 债务清除；
//   step3 正常叙事收尾。
//   断言：ledger.blocking() 为空 → 进叙事轮（narrated）；
// 库面终验（测试内 sqlx 直查或测试后手动 PSQL）：
//   generic_parameter_states 该角色 HP path 的 value 真实 -3（验收 11 的"库中状态真实变化"）。
```
```bash
# 跑法（ignored 测试显式点名；cwd 在 crate 目录——cargo test -p 的既有坑）：
cd crates/trpg-gm && DATABASE_URL=postgres://chatrpg:chatrpg@localhost:54347/chatrpg cargo test -p trpg-gm --test retro_debt_e2e -- --ignored --nocapture
# 库面复核（拿测试打印的 session_id）：
PSQL "select target_id, parameter_path, value_json from generic_parameter_states where session_id='<测试打印的SID>'"
```
- 边界：MockLlm 脚本与断言**不得**依赖勘误记忆的具体措辞（B6 实现细节），只断言结构面（债务存在/回填发生/债务清除/库值变化）。测试文件 ≤400 行。
- waive 出口对照（验收 11 的或然分支）：再加一个用例——回合 2 脚本改调 `waive_obligation(debt_id, reason="叙事中已收回该说法")` → blocking 清空 + memory_events tags 含 gm_waive（库面 PSQL 复核）。

### ② 缓存回归 e2e 半边（验收 13；单测半边在 B1，函数级在 C3 测 5）

```bash
# 同场景连跑 3 回合（输入选纯对话/观察类，不触发 navigate_scene/advance_time——场景与时间不动才是对照面）：
cat > /tmp/c7_cache.txt <<'EOF'
我环顾四周，把现场细节再确认一遍
我蹲下来仔细看地上的痕迹
我把看到的一切在脑子里过一遍
/quit
EOF
run_play call_of_cthulhu_7e call_of_cthulhu_7e.document /tmp/c7_cache.txt c7_cache_run1
SID=$(sid_of c7_cache_run1)
# 证据面 1（tracing）：三回合所有工具轮的三个 hash 各自恒等
grep 'gm agent prompt cache anchors' c7_cache_run1.stderr.log | grep -oE '(prefix_hash|pinned_hash|request_prefix_hash)=[^ ]+' | sort | uniq -c
#   判定：prefix_hash / pinned_hash / request_prefix_hash 各只出现 1 个值（计数=轮数总和）
# 证据面 2（落库，比 tracing 稳）：turns.context_hashes 三行逐字段相等
PSQL "select turn_id, context_hashes from turns where session_id='$SID' order by created_at"
```
- **前提断言**（hash 恒等才有意义）：本把 BP1 真含目录索引、BP2 真含 intents 块——
  - BP1 半边：B1 单测已断言索引块在 Prefix；e2e 行为面 = C5② 里 agent 能感知目录（契约产生）即 BP1 注入的行为证据；
  - BP2 半边：血色公路当前场景若无 scene_mechanics 则 BP2 无 intents 块（fail-closed 正确行为）——此时换 Homecoming 场景（C6① 的 `<HC_MID>`，到含 intent 场景后）重复本节三回合跑批，两个 hash 对照面都要留档；
  - 反向对照（可选加强）：navigate 一次后 pinned_hash **应当**变（SceneStable 语义=场景切换换块），变了反而是对的——记录之，防"恒等"断言被误读为"永不变"。
- 任一 hash 漂移 → 回 B1（索引块字节不稳/PM 档投影把动态值渲染进了 Prefix）或 C3（intents 行序/截断不稳）修复后重跑。

### ③ spec §8 全 14 条验收逐条对账（总报告核心；执行时逐条填"证据/状态"两列）

> 状态词表：**通过** / **通过（带缺口）**（fail-closed 正确退化或概率未观测，备注写明） / **未通过**（指回任务） / **N/A**（写理由）。证据列必须是可复查的实物：测试名、SQL 原文输出、log 文件名、报告路径——**不接受"已实现"三个字**。

| # | spec §8 验收 | 实现位置（任务/锚点） | 测试/SQL 证据（执行时填实物） | 状态 |
|---|---|---|---|---|
| 1 | 六套规则目录生成 + validation_report 记录丢弃及原因 | A2/A3/A6 → A7 跑批 | A7 审计报告路径 + `select ruleset_id, jsonb_array_length(content_json->'mechanics_catalog') from rule_kernels where active=true`（6 行非空） | |
| 2 | CoC sanity_check/temporary_insanity followup 链通 + jump 条目带 when_to_use | A4/A7 | A7 断言记录 + C5 预检②④ SQL 原文 | |
| 3 | 坏条目护栏（tested_parameter 不存在 → 丢弃且报告） | A3 | `cargo test -p trpg-rule-agent` 坏条目注入测试名 | |
| 3a | 覆盖率审计 vs 附录 A 137 条 + 每套 ≥1 非预设类别 + ≥1 纯语义 + ≥1 EngineHook 条目 | A7 | 审计报告（缺失项须在 validation_report 有记录）+ 抽查条目 id 清单 | |
| 4 | watcher 单测（SAN -6→due/-4→无/HP 穿 0→due/无 followup 仍发） | B4 | `cargo test -p trpg-mechanics` 测试名×4 | |
| 5 | 债务门控单测（due 不清不进叙事轮/waive 放行+落账/轮耗尽进下回合 BP3） | B6 | `cargo test -p trpg-gm` 测试名×3 | |
| 6 | lookup_mechanic / roll_check(mechanic_id) 继承绑定 | B2 | `cargo test -p trpg-gm` 测试名 + schema_stability 照绿 | |
| 7 | e2e CoC SAN→疯狂全链 SQL 可查 | C5① | C5 终验 SQL 四步原文（或半链+未观测记录） | |
| 8 | e2e 跳坑感知（tested_parameter 与目录条目一致，不写死技能名） | C5② | JUMP_ID/JUMP_PARAM 语义选取记录 + 契约 SQL 原文 | |
| 9 | e2e Homecoming 切线缆 effect_policy 强制执行落库 | C6①（实现 C2/C3/C4） | world_events(scene.policy)/object_instances/scheduled_events SQL 原文 + 分支逐条对账表 | |
| 10 | e2e Triangle chaos 累积+语义投影+行为反映（三件套） | C6②（实现 A7/B3/B4） | 递增 SQL + transcript 摘录 + evaluator 结论备注 | |
| 11 | e2e 追溯债务闭环（InventedEffect→债务→补 apply_effect→库变化） | C7①（实现 B6/B7） | retro_debt_e2e 测试名×2 + 库面 PSQL 原文 | |
| 12 | 成功度三件套（①band 补全护栏 ②fumble max_of 机械落库 ③语义投影分档） | ①A4 ②A5 ③B3 + C5③ | ①A7 报告+C5 预检③ ②定性 SQL+（A5 测试名 或 fumble 实测）③check_results band+叙事对照 | |
| 13 | 缓存回归（目录注入后跨回合 prefix/pinned hash 不变） | B1（单测）+ C7②（e2e） | B1 测试名 + c7_cache uniq 输出 + turns.context_hashes SQL 原文 | |
| 工程 | 文件 ≤400 行 / 零 per-ruleset 硬编码 / 新结构全 `#[serde(default)]` | 全任务 | 下方三查命令输出 | |

**工程约束三查（照抄，输出贴进报告）**：
```bash
# ① 新文件行数（File Structure 的 Create 清单全列 + 本 section 新增的 scene_mechanics.rs/retro_debt_e2e.rs）
wc -l crates/trpg-model/src/mechanics.rs crates/trpg-rule-agent/src/reader/mechanics_compile.rs \
  crates/trpg-rule-agent/src/reader/mechanics_finalize.rs crates/trpg-rule-agent/src/bin/mechanics_proto.rs \
  crates/trpg-rule-agent/src/reader/scene_mechanics.rs crates/trpg-mechanics/src/watcher.rs \
  crates/trpg-gm/src/obligations.rs crates/trpg-gm/src/scene_policy.rs crates/trpg-gm/src/tools/mechanic.rs \
  crates/trpg-gm/tests/retro_debt_e2e.rs    # 全部 ≤400
# ② 零 per-ruleset 硬编码（命中只允许出现在测试 fixture/注释/文档；代码分支命中=未通过）
grep -rn -iE "call_of_cthulhu|cthulhu|cyberpunk|triangle_agency|sword_world|dnd|d&d|coc[^a-z]" \
  crates/trpg-model/src/mechanics.rs crates/trpg-rule-agent/src/reader/mechanics_*.rs \
  crates/trpg-rule-agent/src/reader/scene_mechanics.rs crates/trpg-mechanics/src/watcher.rs \
  crates/trpg-gm/src/obligations.rs crates/trpg-gm/src/scene_policy.rs crates/trpg-gm/src/tools/mechanic.rs
# ③ serde 向后兼容回归（A1/C1 的老 JSON fixture 测试 + 全 workspace 测试矩阵收口）
cd crates/trpg-model && cargo test -p trpg-model
cd ../trpg-rule-agent && cargo test -p trpg-rule-agent
cd ../trpg-mechanics && cargo test -p trpg-mechanics
cd ../trpg-gm && cargo test -p trpg-gm
cd ../trpg-runtime && cargo test -p trpg-runtime && cd ../.. && cargo build -p trpg-cli
```

**可观测技术债清单收口（报告固定章节；骨架 C7③ 点名项 + 执行中新增项）**：
| 债项 | 来源 | 可观测面 |
|---|---|---|
| SessionEnd/DevelopmentPhase 无引擎事件点（仅原语支持） | B5 | A3 降级记录在 validation_report |
| party/world owner_kind 写路径维持现状（投影侧已通用） | B3 | validation 记录 |
| band trigger 未解析出的规则集（若有） | A4/A5 | validation_report + 对账表 12② 备注 |
| verifier 子串扫描回退路径 | B7 | finding detail 前缀 fallback:substring_scan（C7① 已断言） |
| check_match regex 兼容回退（契约无 mechanic_id 时） | 护栏 §3.5.1 | 账本/validation_report 标注 |
| （执行中发现的新债逐条追加） | | |

### 验证（任务收口 = 二期收口）
- 验收 11：retro_debt_e2e 两用例过 + 库面 PSQL 原文；验收 13：两证据面 hash 恒等 + 前提断言留档。
- 对账表 14 行 + 工程行全部填实（无空格无"已实现"），状态列出现「未通过」则二期不收口——指回任务修复后重跑该行证据。
- 总报告落 `docs/规则感知GM二期验收报告_2026-06-10.md`（中文），含：对账表、三链计时表汇总（vs 一期基线，回答 spec §9 风险 4）、技术债清单、evaluator 对照结论摘要。

---
