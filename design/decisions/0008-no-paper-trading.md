# ADR 0008 — P1 不做 paper trading

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0009](0009-portfolio-as-research-context.md)（portfolio 重定位为投研参考视图）

## Context

之前的设计讨论里（handoff §5 延伸题 #6），paper trading 是否在 P1 实现是个开放问题。最初的倾向是"做"——agent 输出投资动作（仓位 / 止损 / 期限）后，可以在虚拟账户里"执行"，跟踪虚拟收益、做 mark-to-market、触发复盘。

深入设计时发现 paper trading 是一个**自带巨大 surface area** 的子系统：

- **执行时点**：agent 出 decision → 自动开仓 vs 用户 confirm 才开仓？前者破坏"投资动作必须人审"原则，后者增加 UI 流程
- **价格模型**：用什么价？提交时点的中间价？bid/ask？是否加滑点模型？是否模拟限价单 / 市价单的差异？
- **账户结构**：起始余额 / 多账户 / 杠杆 / 保证金 / 货币 / 跨市场？
- **结算频率**：实时 mark-to-market 还是定时 / 事件触发？分红 / 拆股 / 配股 / 股权激励 / 私有化怎么处理？
- **复盘自动化**：到期是否自动触发复盘任务？是用 cron 还是事件流？
- **数据真实性**：行情 tick 是订阅了实时还是延迟数据？历史回填的对齐？

每一条都是 1-2 周工作量，叠加是 6-10 周。这把 P1 schedule 直接拖到 2-4 个月开外。

项目所有者明确决定：

> "我觉得还是不要 paper trading 了，至少现在先不做。portfolio 更多是做投研时给 agent 的参考。"

## Decision

**P1 不做 paper trading。**

具体含义：

- ❌ 不实现"agent 提交 decision → 虚拟账户开仓"的自动执行流
- ❌ 不实现实时 / 定时 mark-to-market
- ❌ 不实现公司动作（分红 / 拆股 / 配股）模拟
- ❌ 不实现回测 / 历史模拟
- ❌ 不实现 PnL 曲线 / 收益率统计 / sharpe / drawdown

**保留**：
- ✓ Portfolio panel 进 P1（[ADR-0009](0009-portfolio-as-research-context.md)）——但 portfolio 是用户手工录入或同步真实账户的**只读快照**，不是模拟交易状态
- ✓ Decision 仍然是 P1 核心产出（agent 输出仓位 / 止损 / 期限 / 复盘 schedule）
- ✓ Decision 状态机：`draft → confirmed → closed | superseded`（人工驱动，不是市场触发）
- ✓ Review（复盘）仍然进 P1，但触发是手动 / cron 提醒，不是 PnL 触发

## P1 接口预留（不实现）

为未来可能加 paper trading 留接口形态，但不实现 driver：

```rust
trait PortfolioOps {
    async fn open_position(&self, decision: &Decision) -> Result<Position>;
    async fn close_position(&self, position_id: &str) -> Result<Trade>;
    async fn mark_to_market(&self, ticker: &str, price: f64) -> Result<()>;
}

// P1 默认实现：什么都不做的 NoopPortfolioOps
// 未来可以加 PaperPortfolioOps（虚拟账户）/ BrokerPortfolioOps（真实下单）
```

trait 的存在只是为了**未来加新 driver 时不用改 agent core 代码**——不增加 P1 实施工作量（写 trait + 一个空实现 = 30 行代码）。

## Consequences

### P1 范围大幅收紧

- 没有虚拟账户余额管理
- 没有 PnL 计算
- 没有 trade history 表（只有 decisions / holdings / reviews）
- 没有"持仓视图随行情自动刷新"的实时性需求（持仓是用户录入的快照，刷新由用户主动触发）

这些不实现节省的 P1 工作量大约 6-10 周，schedule 大幅前移。

### Portfolio Panel 的语义改变

Portfolio 不再是"模拟交易后的状态"，而是"**用户当前真实持仓的镜像，给 agent 做投研上下文**"：

- 数据来源：用户手工录入 / CSV 导入 / 未来可加券商 API 同步
- 更新频率：用户主动更新（"我刚加仓 NVDA 1000 股")，agent 不能自动改
- agent 用法：把当前 portfolio 注入 system prompt，做新决策时"参考用户已有仓位"

详见 [ADR-0009](0009-portfolio-as-research-context.md)。

### 投资动作的"落地"由用户在 leek 之外完成

P1 流程：
1. agent 输出 decision draft（仓位 / 止损 / 期限 / 复盘 schedule）
2. 用户在 chat-canvas 看到 DecisionDraft panel，可以编辑 / confirm
3. confirm 后 decision 进 `decisions` 表，status = `confirmed`
4. 用户去自己真实的券商 app 下单
5. 下单后用户**回到 leek 主动更新 portfolio**（"我已按 decision 下单了"），portfolio 反映最新持仓

这条流程**不需要 leek 模拟执行**——投资动作的执行边界明确在 leek 之外（用户的真实账户），leek 只做研究 / 决策 / 跟踪。

### Review 的触发仍然有意义

虽然没有 PnL，review 仍可被触发：
- decision 的 `review_schedule_json` 字段存"打算什么时候复盘"的日期数组
- gateway 的 cron loop 检查到期 → 推送 reminder（前端 panel / 通知）
- 用户主动开 review → agent 拉取 decision 上下文 + 当时的 corpus refs + 最新行情 + 当前 portfolio → 生成复盘草稿
- 用户编辑 / confirm review → 进 `reviews` 表

完全不依赖 paper trading。

## Alternatives Considered

### 做最简版 paper trading（被否）
- 即使最简版（执行用 mid price + 不处理公司动作 + 手动 mark），也至少 2-3 周工作量
- 边界容易扩散：用户用着会很快要求"加滑点"、"分红重投"、"多账户"
- P1 阶段优先打通"用户可以用 leek 做投研"的核心闭环，paper trading 是锦上添花

### 接真实券商 API（推迟更久）
- 真实下单是合规问题（license / 风险声明 / 操作审计 / 二次确认），P1 不可能做
- P3+ 议题

### 仅记录 decision 不跟踪 portfolio（被否）
- 项目所有者明确 portfolio panel 进 P1，作为投研参考
- portfolio 与 decision 是不同维度（持仓快照 vs 决策事件），都需要

## 重新评估的触发条件

- **触发 1**：用户在使用过程中明确表达"我想看 decision 跑下来的虚拟收益曲线"
- **触发 2**：核心闭环（research → decision → review）已稳定运行 1-2 个月，团队有富余精力
- **触发 3**：找到有效的"防止 surface area 爆炸"的设计模式（如复用某成熟 paper trading 引擎、明确划界"我们只支持股票现货 mid price"）

任一触发出现，重开 ADR 评估是否新增 paper trading。
