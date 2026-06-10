# ModuleGraph 懒化/Frontier 返工 + P1/P2/P3/P5 计划

> **执行**:subagent-driven,分波并行。基于 `2026-06-09-module-graph-upgrade*` 的成果返工(那版全局边/前台 gleaning 太慢:CoC 首次 232s、87 场景仅 17 边 77 孤立)。
> 护栏:语义优先零硬编码 · fail-closed · 文件 ≤400 · 复用。无 git。

## 目标架构(已与用户敲定)

- **前台·最小可玩**:TOC 定向 → Pass A 单遍骨架(TOC级 stub,**不 gleaning**)→ 定入口 → **一次性深抽入口**(切页,非 ReAct)+ 闭包 + **入口局部出口**。交付。
- **后台·铺底**:gleaning **只补 stub**(完成可导航地图,无深抽)。
- **play/frontier**:scene-nav 到场 → 一次性深抽该场景 + 算出口 + **前探其衔接场景(一跳)**。**没去的场景永停 stub,零深抽 token**。
- 深抽量 ∝ 玩家路径,不 ∝ 模组大小。

## 返工任务

### RW1 一次性深抽(P1)— module_reader_loop.rs `deep_extract_scene_in_place`
重写:若场景有 `page_start/end` 且 `ctx.sidecar_text` 有 → **切那几页(复用 `chargen_compile::read_layout` 切页)+ 一次 `complete_with_tools`/submit**(submit_deep schema:scene{read_aloud,gm_notes,referenced_*_ids,**exits**} + entities),**不走 ReAct 循环**。无页码/无 sidecar → 回退现有 ReAct loop。deep prompt 含 **RW8 锚句提示** + **候选出口列表(RW2)**,要求从候选里选 exits 并带 source_anchor。apply_deep_to_node 填 links。

### RW2 候选衔接场景(懒边)— module_graph_edges.rs
新纯函数 `candidate_neighbors(scenes, idx) -> Vec<&str>`:TOC 局部(skeleton 顺序相邻 ±N、或同 chapter/node_type 段)∪ 实体共享者(复用 `bridge_edges` 取含 idx 的对)。去重。喂给 RW1 deep prompt 当出口候选。**删全局 `apply_bridge_edges` 调用 + 全局 `pass_c_clue_edges`**(出口改在深抽里局部算)。`bridge_edges` 保留作候选生成。

### RW3 前台瘦身(去 gleaning)— module_reader.rs `run_module_reader` + module_graph_build.rs
前台 Pass A **删 gleaning 回环**(移后台);run_module_reader = Pass A 单遍骨架 → dedup(cheap)→ 定入口 → RW1 一次性深抽入口(候选出口)。**删 build_module_graph 里的全局边 pass**;保留 dedup;validate_graph 降**诊断**(只 log,RW7)。

### RW4 模型分层(P2)— module_reader.rs
骨架/gleaning 用 `TRPG_MODULE_SKELETON_MODEL`(env,默认沿用主模型)构建的便宜 client(仿 `build_compiler_llm`);深抽用主 client。run_module_reader 内建 skeleton client 传给 Pass A/gleaning。

### RW5 CoC 页码(P3)— trpg-ingest semantic_units 注入页锚(独立 crate,可并行)
生成 semantic_units 时把所在 `# Page N` 页码写进 unit 的 content_text 开头或 page_numbers,使 reader 能给每个场景 stub 附 page_start/end → 解锁 RW1 对 CoC 生效。fail-closed:无页锚不强造。

### RW6 Frontier 前探 — trpg-api `scene_navigator` + 替换 P5 全量续抽
- scene-nav 到场 target:一次性深抽 target(RW1)+ 算 target 出口 + **前探 target 的衔接场景(逐个 `extract_module_scenes(only=exit)`,一跳)**。
- **删/停用** P5 自动"续抽所有 SkeletonOnly";改为交付后**后台 gleaning 补 stub**(若做后台 job)+ frontier 前探。
- gleaning 后台化:parse_module 交付后排一个 stub-only gleaning job(或并入现有续抽 job 改造)。

### RW7 validator 诊断化 — module_graph_build.rs
去掉 `!ok → re-gleaning` 的质量门;validate_graph 只 `tracing::info!` 连通性报告(解析时图稀疏=预期)。

### RW8 read_aloud 锚句提示(P5)— RW1 的 deep prompt(零成本)
deep prompt 加:"念白优先按锚句抓(如『read, or paraphrase the following text:』或第二人称『你们』的 boxed 段);无则 null,绝不编造"。

### RW9 e2e — 重抽 CoC/Vault
测**首次抽取时间**(应从 232s 大降)、入口出口质量(candidate→exits 带 anchor)、frontier 深抽只沿路径。

## 波次(文件冲突安全)
- Wave1 并行:RW5(trpg-ingest,独立)‖ RW1+RW2+RW8(module_reader_loop + module_graph_edges 的深抽/候选)。
- Wave2:RW3+RW4+RW7(module_reader + module_graph_build 前台瘦身/分层/诊断)。
- Wave3:RW6(trpg-api frontier + scene_navigator)。
- Wave4:RW9 e2e(controller)。
