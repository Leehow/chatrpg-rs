# Binding Takeover 竖切（检定结算）设计 spec

> 2026-06-17。目标：让已存在但处于**影子态**的 BindingResolver **第一次真正驱动一次结算**，
> 在一条能力路径（检定 compare）上打通 `kernel facet → BindingPlan → ExecutionTier 路由 → CapabilityExecutor → 真结算`，
> 证明 Source-grounded JSON Asset Runtime 范式能 load-bearing，而非永远当观察者。

## 背景与现状（已 ground-truth）

- `AssetEnvelope/AssetFacet/ExecutionTier`（`trpg-model/src/asset.rs`）、`BindingPlan/CapabilityRegistry`
  （`trpg-runtime/src/binding.rs`）**类型都已建**。
- 但 `shadow_bind_with_kernel` **只在 `trpg-gm/src/turn_trace.rs:88` 被调一次**，产出只进 Flight Recorder
  （binding_trace），**不进结算**。`ExecutionTier` 只在 CLI coverage 显示。`AssetEnvelope` **无生产者**。
- 结算咽喉：`ContestService::resolve_outcome`（`trpg-contest/src/lib.rs:18`）做 compare 分发；
  runtime 单点 `resolve_outcome_with_opposition` 包它（被回合多路径调用）。
- `facets_from_kernel` 已把 `kernel.dice_core.compare` → `CAP_CHECK_ROLL_UNDER/MEET_OR_BEAT/COUNT_FACES`
  （`check_capability_for_compare`，确定性、无 LLM、无规则集名）。

**结论：竖切不必先迁 asset 生产者**——binding 已能从 kernel facet 解析出检定 capability。这条路风险最小。

## 竖切设计

### 1. CapabilityExecutor（检定族，3 个）
新增 trait + 注册表执行器，**每个 executor 包既有 `resolve_outcome` 逻辑、不改算法**：
```
trait CheckCapabilityExecutor { fn capability_id(&self)->&str;
    async fn execute(&self, contract, roll, opposition) -> CheckOutcome; }
```
`CAP_CHECK_ROLL_UNDER / MEET_OR_BEAT / COUNT_FACES`（opposed 复用 CAP_CHECK_OPPOSED）各一个，内部**调用现有 resolve_outcome**。
即：indirection 是新的，算法是旧的 → 天然等价。

### 2. 钩子点：`resolve_outcome_with_opposition`（runtime 单点）
- flag OFF（默认）→ 走现路径（直接 `ContestService::resolve_outcome`），**零行为变更**。
- flag ON → 取该检定 need 的 `BindingPlan`（kernel facet 已能产）；
  - verdict==`Exact` 且 capability 命中 → 经 CapabilityExecutor 执行（仍调旧 resolve）。
  - 其余（Partial/Guided/SourceOnly/Unsupported）→ **fail-closed 回退现路径**（本竖切检定都是 Exact，回退分支此期不触发，但留作 tier 路由的第一处真实分叉）。

### 3. Flag + 等价证明（先证后翻）
- env `TRPG_BINDING_TAKEOVER` 默认 OFF。
- 黄金 e2e 双分支：同 TurnRequest + 同骰种 + 同 assets，flag ON vs OFF → **dice/检定结果 + state patch 逐字节等价**
  （手法同 `TRPG_PLAYER_REPORTED_ROLL_TOTALS` 的确定性 e2e；见 [[gate-path-effect-policy]]）。
- 等价过了再把默认翻 ON（行为不变但路径换成 binding 驱动）。

### 4. ExecutionTier 第一次 load-bearing
路由按 tier 分叉（Exact→执行 / 非 Exact→回退或 guided），**tier 从此不再只是 CLI 显示字段**。

## 范围边界（本竖切只做这些）
- ✅ 检定 compare 结算经 binding 驱动 + tier 路由 + 等价证明。
- ❌ 不做：asset 生产者迁移（parser 仍出 ContextBlock/kernel）、resource/effect/state 的 binding 化、
  TurnCommit 一等对象、PlayabilityManifest L0-L5、三图防剧透。**这些是后续竖切**（证明范式后再铺）。

## 验收
1. flag OFF：535+ 现有测试全绿、两 e2e（CoC roll_under / Cyberpunk meet_or_beat）逐字节等价基线不变。
2. flag ON：同样两 e2e 逐字节等价 OFF（铁证零行为变更）。
3. `trpg explain` 的 binding_trace 显示该回合检定**经 capability 执行**（verdict=Exact, tier=ExactExecution, capability=CAP_CHECK_*）。
4. 注入一个伪造 compare=未知值 → 回退现路径 + warning（fail-closed 可观测）。
5. 零规则集硬编码（两守卫绿）、文件 ≤400 行。

## 真正要你（架构师）拍板的 2 点
1. **首个能力选检定 compare**（vs 更简单的 resource.delta）。我选检定：它是最中心、最能证明"binding 能驱动核心结算"的路径，且映射已现成。
2. **flag-gated 渐进 + 等价证明后翻默认**（vs 直接接管）。我选 flag-gated：符合零回归铁律，可逐字节自证。
