# ADR 0010 — 单 Agent + On-Demand Subagent (Map-Reduce)

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0005](0005-self-implemented-harness.md)（自实现 harness）；[0003](0003-corpus-as-static-resource.md)（corpus 作为静态资源决定了 multi-agent 的前提条件不成立）
- **相关文档**：[`interaction-model.md`](../interaction-model.md)（用户视角的 manager + team 模型）

## Context

在确定 L.E.E.K 的产品定位为"manager + team"（用户作为基金经理，agent 作为研究团队）之后，出现一个核心架构问题：**这个"团队"是真 multi-agent，还是单 agent？**

三个候选实现：

| Option | 用户体验 | 实际架构 | P1 工作量 | 风险 |
|--|--|--|--|--|
| **A. 完整 Multi-Agent** | 真团队（多 persistent specialist agent 并行 / 串行 / handoff） | 协调 agent + N 个 specialist + 共享 state + handoff 协议 + 并发调度 | +6-8 周 | 调试难度极高 / state 一致性 / 死锁 |
| **B. Role-Switching 单 Agent** | 看起来像团队（同一个 agent 自称不同 role） | 单 agent，system prompt 按 phase 切换 | +1-2 周 | 用户感知不到团队感 |
| **C. 单 Agent + On-Demand Subagent** | manager 给 lead 下任务，lead 内部按需调度临时小组 | 单 main agent (coordinator) + 短生命周期 subagent (map-reduce) | +1-2 周 | 需要明确 subagent 的接口边界 |

项目所有者明确选择 C，并给出关键判断：

> "现阶段我们就是单 agent，因为多 agent 真正要产生效果，我们需要一个大得多的 corpus，甚至从根本上就是多套理论互相 challenge 或者印证。否则强行拆多 agent，效果不是这么明显，上下文隔离可以用 coordinator + subagent 这样的 map-reduce 模式实现。"

这个判断的核心点：

1. **multi-agent 出效果的前提是"多套理论 / 多种视角"**——如果 corpus 只有一套主流投资哲学，多个 specialist agent 本质上是同一脑子在不同 prompt 下推理，差异很小
2. **当前 corpus 体量不够**——Buffett / Munger / Dalio 思想为主的 ~300 篇，还不构成"多理论框架"
3. **上下文隔离的实际需求 ≠ multi-agent 协调**——map-reduce 风格（spawn 一个干净的 subagent 跑一段独立逻辑、返回结果）就能解决 80% 的"context 太满"问题

## Decision

**P1 采用 Option C：单 main agent (coordinator) + on-demand subagent (map-reduce)。**

### 架构

```
                    User (Manager)
                         │
                         │ task
                         ▼
              ┌─────────────────────┐
              │   Main Agent        │
              │   (Coordinator)     │
              │                     │
              │  · 主 context       │
              │  · 主 scratchpad    │
              │  · 直接调工具        │
              │  · 写 vault         │
              │  · 撰写 deliverable │
              └──┬─────────────┬────┘
                 │ spawn       │ spawn
                 │             │
                 ▼             ▼
          ┌──────────┐   ┌──────────┐
          │ Subagent │   │ Subagent │   ... (并行 or 串行)
          │ #1       │   │ #2       │
          │          │   │          │
          │ 独立 ctx │   │ 独立 ctx │
          │ 限定工具 │   │ 限定工具 │
          │ 限定步数 │   │ 限定步数 │
          └─────┬────┘   └─────┬────┘
                │              │
                │ structured   │ structured
                │ return       │ return
                ▼              ▼
              [merged into main agent context]
```

### Subagent 的语义

- **生命周期短**：spawn → run → return → terminate；不保留状态
- **独立 context**：subagent 不继承主 agent 的 message 历史；只看到主 agent 给它的 scope + 输入
- **工具受限**：subagent 调用的工具是主 agent 给它的子集（不能写 vault、不能 spawn 其他 subagent、不能改 task 状态）
- **预算受限**：spawn 时给 max_turns / max_tokens / max_duration，超过则强制返回（含部分结果）
- **结构化返回**：返回值是 schema 化的（不是流式聊天）；主 agent 拿到结果后决定怎么 merge

### Spawn 协议

```rust
trait Subagent {
    fn spec(&self) -> SubagentSpec;
    async fn run(
        &self,
        scope: SubagentScope,
        input: SubagentInput,
    ) -> Result<SubagentOutput>;
}

struct SubagentScope {
    name: String,                       // "valuation_dcf" / "news_summary" / ...
    goal: String,                       // 自然语言任务描述
    allowed_tools: Vec<String>,         // ["quote", "financials"] (子集)
    max_turns: u32,                     // 默认 5
    max_tokens: u32,                    // 默认 8000
    max_duration_sec: u32,              // 默认 60
    return_schema: serde_json::Value,   // JSON Schema 描述期望返回的字段
}

struct SubagentInput {
    context: String,                    // 主 agent 给的简短背景
    parameters: serde_json::Value,      // structured 参数
}

struct SubagentOutput {
    success: bool,
    result: serde_json::Value,          // 符合 return_schema
    summary: String,                    // 主 agent 阅读的简短总结
    tokens_used: u32,
    turns: u32,
    duration_ms: u64,
    error: Option<String>,
}
```

### 何时 spawn subagent

主 agent 在以下场景**主动选择** spawn subagent（不是用户指令）：

1. **并行可分解任务**：如 "对比 A / B / C 三个 ticker 的财务" → spawn 3 个 subagent 各跑一个 ticker 的财务分析，结果 merge
2. **大量 token 消耗的探索性工作**：如 "深读这 10 篇研报提取要点" → spawn 一个 subagent 处理，主 agent 不被 10 篇研报淹没 context
3. **需要"clean room"的子推理**：如 "不带先验地评估这个 ticker 的护城河" → spawn 一个不知道当前 portfolio 的 subagent，避免 anchoring bias
4. **试探性 tool 调用**：如 "用各种参数试一下这个估值模型，找最稳的" → spawn subagent 跑参数 sweep

### Subagent 在 UI 中的可视化

- ReasoningDAG 中 subagent 的工作显示为**子分支**（不同颜色 / 缩进）
- 用户能看到"主 agent 调度了 N 个 subagent，每个完成情况如何"
- 但用户**不能直接和 subagent 对话**——若想干预，要通过对主 agent 说

详见 `frontend/panels.md`（修订后）的 ReasoningDAG / TeamView 章节。

## Consequences

### 实施层面

- **Harness 复杂度可控**：subagent 本质是"主 agent 的一个 tool 调用"——`spawn_subagent(scope, input)` 是个工具，主 agent 调用它就像调任何其他工具。不需要并发协调死锁等复杂机制。
- **State 一致性简单**：所有持久化（vault 写入）都经过主 agent。subagent 只读不写。
- **调试可观测**：每个 subagent run 都持久化（`vault.subagent_runs` 表，含 input / output / duration / error），随时回放。

### 用户体验层面

- 用户感受到的是 manager + lead + on-demand strike teams 的工作流
- 用户**不需要管理 subagent**——它们是 lead 的内部资源
- 长任务的进度感很强：用户能看到主 agent 派出 N 个 subagent → N 个 subagent 各自工作 → merge 成最终答案

### 性能层面

- **并行加速**：subagent 真并行（多个 LLM 请求同时发出），long task 的 wall-clock 时间显著缩短
- **Token 预算控制**：每个 subagent 有 max_tokens 上限，主 agent 不会被某次失控的子任务烧光预算
- **缓存友好**：subagent 的 system prompt 是模板化的，可以利用 LLM 的 prompt caching

### 局限性

- 没有"specialist 视角"——所有 subagent 用同一个底层 LLM，只是 system prompt 不同。不会出现"价值投资派 vs 趋势派" challenge 彼此的真效果
- 真要 multi-perspective challenge，需要 corpus 增长 + 多套理论框架建立后，再升级到真 multi-agent

## Alternatives Considered

### A. 完整 Multi-Agent（被否）

理由：
- corpus 体量不支持 multi-theory 真效果（用户原话）
- 实施复杂度极高，P1 schedule 直接 +6-8 周
- 调试 / state 一致性 / 死锁等运维成本超出 P1 承受
- 没有实证证据表明 multi-agent 在投研场景下显著优于单 agent + subagent

**重新评估的触发条件**：
- corpus 扩展到包含 ≥ 3 套独立投资理论框架（如价值 / 趋势 / 量化各 100+ 篇深度内容）
- 用户在使用中明确感到"我希望听到不同流派的对立观点"
- subagent 在某些场景下表现明显不足（如 cross-validation 任务做不动）

### B. Role-Switching 单 Agent（被否）

理由：
- 用户感知不到团队感——只是同一个 agent 在改 prompt
- 没有 context 隔离能力——主 context 持续被 phase 切换的内容污染
- 用户原话明确："上下文隔离可以用 coordinator + subagent 这样的 map-reduce 模式实现" → 期待的是真 isolation，不是 prompt switch

### C. 单 Agent 不带 subagent（被否）

理由：
- 长任务时主 agent context 容易爆（深读 10 篇研报、跑参数 sweep 等场景）
- 没有并行加速能力
- 但作为 P0 起点（最简版本）仍可行——如果 P1 schedule 紧，可以推迟 subagent 到 P1.5

## P1 实施边界

P1 必做：

- ✓ Main agent 的 `spawn_subagent` tool（注册到 ToolRegistry）
- ✓ SubagentRunner 实现（独立 context + 工具子集 + 预算限制）
- ✓ `vault.subagent_runs` 持久化
- ✓ Reasoning DAG 中可视化 subagent 分支
- ✓ ≥ 2 个内置 subagent specs（如 `valuation_dcf` / `news_summary`），让主 agent 可以即开即用

P1 不做：

- ❌ 用户直接选 "用某 subagent 处理"——subagent 选择由主 agent 决策
- ❌ Subagent 之间互相 spawn / 通信
- ❌ Subagent 对 vault 的写入
- ❌ Subagent 的 persistent memory（完全无状态）

## 升级路径（未来 multi-agent）

当触发条件成立、要升级到真 multi-agent 时，**当前架构允许平滑过渡**：

1. **Subagent → Persistent Specialist Agent**：把 spec 改成 persistent 实例，加 long-term memory
2. **Spawn 协议 → Routing 协议**：主 agent 不再 spawn 而是 route 任务给 specialist
3. **Single Coordinator → Multi-Lead**：可能引入 "research lead" + "execution lead" 等多个 lead

这些都是**增量升级**，不破坏 P1 的代码骨架。Subagent 的 trait 设计是面向未来的：scope / input / output 这套 interface 可以直接用于 persistent agent。

## 验证标准

- 主 agent 能在 5 行代码内 spawn 一个 subagent 并拿到 structured 结果
- 单个 task 中并行 spawn 3+ subagent 时，wall-clock 时间相比串行减少 ≥ 50%
- Subagent run 完整可观测（input / output / token / duration 都进 vault）
- Reasoning DAG 中 subagent 分支视觉清晰，用户能 distinguish 哪一段是 subagent 做的
- Subagent 出错（超时 / token 超限 / tool 报错）时 graceful degrade，主 agent 拿到部分结果继续工作
