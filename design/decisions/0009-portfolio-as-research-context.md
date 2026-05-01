# ADR 0009 — Portfolio 作为投研参考视图

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0008](0008-no-paper-trading.md)（不做 paper trading 是这个决策的前置条件）

## Context

Portfolio 在 L.E.E.K 中的角色一直是个开放问题。最初的设计倾向是 paper trading 子系统的状态视图——agent 提交 decision → 虚拟开仓 → portfolio 反映虚拟持仓 + PnL。

随着 [ADR-0008](0008-no-paper-trading.md) 决定不做 paper trading，portfolio 的角色需要重新定义。否则就要砍掉 portfolio 这个 panel，但项目所有者明确 portfolio panel 仍然进 P1：

> "portfolio 更多是做投研时给 agent 的参考。"

这句话给了 portfolio 在 P1 的新定位。

## Decision

**Portfolio = 用户真实持仓的镜像，给 agent 做投研上下文。**

### 核心语义

- Portfolio 是用户**真实账户的持仓快照**，由用户在 leek 之外的真实券商账户下单后**主动同步进来**
- Portfolio 不是 leek 模拟出来的状态——leek 不知道执行价、不计算 PnL、不处理公司动作
- agent 把 portfolio 当作 **system prompt 注入的上下文**，新做决策时知道"用户已有 X 仓位"
- agent **不能自动改 portfolio**——portfolio 的 mutation 只能由用户主动操作触发

### 数据来源（P1）

P1 支持两种录入方式：

1. **手工录入**：用户在 Portfolio panel 直接添加 / 编辑 / 删除持仓行（ticker、qty、avg_cost、备注）
2. **CSV 导入**：用户上传券商导出的持仓 CSV，leek 解析后填充

P1 不做：
- ❌ 实时券商 API 同步（合规复杂，P3+）
- ❌ 多账户 / 多市场（P2+）
- ❌ 杠杆 / 保证金（永远不做 / 视用户需求）

### 数据形态（vault）

```sql
CREATE TABLE holdings (
    user_id TEXT NOT NULL,
    snapshot_at TEXT NOT NULL,   -- ISO8601；同一时刻多行 = 一个完整快照
    ticker TEXT NOT NULL,
    qty REAL NOT NULL,
    avg_cost REAL,               -- 用户录入的平均成本（可空）
    notes TEXT,
    PRIMARY KEY (user_id, snapshot_at, ticker)
);
```

每次用户更新 portfolio = 写一个新 `snapshot_at` 的完整快照。**不就地修改老快照**——这样可以查询任意历史时刻的 portfolio 形态（"我做这个决策时持仓什么样？"）。

### Agent 上下文注入

Agent loop 在 build context 时，把**最新 snapshot 的 portfolio** 注入 system prompt：

```
<user_portfolio snapshot_at="2026-05-01T14:00:00Z">
- AAPL: 200 shares @ avg $145
- NVDA: 50 shares @ avg $420
- BABA: 1000 shares @ avg $90
</user_portfolio>
```

agent 在生成 decision 时被 prompt 自然引导考虑"已有仓位 / 集中度 / 行业暴露"。

### Portfolio Panel 的能力（P1）

- 列表视图：tickers + qty + avg_cost + 当前价（实时拉取）+ 浮动盈亏（基于用户录入的 avg_cost）
- 编辑：行内编辑 / 添加 / 删除
- CSV 导入：拖拽 / 选择文件
- 历史快照：dropdown 切换"看 X 时刻的 portfolio"
- 不做：交易记录 / PnL 曲线 / 收益统计 / sharpe 等量化指标

### Decision 与 Portfolio 的关联

- decision 在确认（status = confirmed）时**不自动改 portfolio**
- 用户在真实券商下单后主动更新 portfolio
- decision 表里有 `confirmed_at` / `closed_at` 时间戳，可以与 portfolio snapshot 历史对照查询（"这个 decision confirmed 之后我的 portfolio 怎么变了"）——但这是查询能力，不是数据耦合

## Consequences

### Agent 决策质量提升

知道用户已有持仓后，agent 可以：
- 避免推荐已重仓的标的
- 提示集中度风险（"你已经在科技股配置 65%，再加 NVDA 会到 78%"）
- 推荐对冲 / 分散思路
- 复盘时对照"决策时点的 portfolio"评估当时的判断

不知道这些信息时，agent 只能做通用分析，无法贴合用户实际状况。

### Portfolio 不需要实时性

- 用户主动同步驱动，不是事件驱动
- Portfolio panel 显示的"当前价"是实时的（拉行情 tick），但**持仓数据本身**是用户录入时的快照
- 因此 panel 的状态机简单：snapshot 数据稳定 + 价格层流式刷新

### CSV 导入是 P1 必需

手工录入太繁琐（用户可能持仓 30+ 标的）。最常见的录入方式是从券商 app 导出持仓 CSV / xlsx，leek 解析。

P1 支持几个主流券商格式：
- 富途 / 老虎 / IB（美股 / 港股）
- 雪球（国内股民常用记账工具）
- 通用列名识别（`ticker / symbol / qty / shares / price / cost`）

详细 CSV schema 在 `p1-spec/tools.md` 里展开。

### Portfolio 的私密性

Portfolio 是用户最敏感的数据之一（持仓暴露财力 + 投资偏好）：
- 永远不离开 vault
- 不打 log 不进 telemetry
- agent 把 portfolio 注入 prompt 时确实会发到 LLM provider —— 这是用户必须知道的隐私边界
- P1 提供 user 配置项 `disable_portfolio_in_context`，关闭后 agent 看不到 portfolio（但回答质量会下降）

## Alternatives Considered

### Portfolio 不进 P1（被否）
- 项目所有者明确要进
- 投研体验差太多——agent 看不到用户实际状况只能给通用建议

### Portfolio 自动从券商 API 同步（推迟）
- 各券商 API 资质 / 合规 / 安全性各异
- 国内券商基本不开放散户 API（要走机构通道）
- 富途 / 老虎 / IB 有 API 但 P1 不接（工作量大 + 接 1 家不接其他家又有不公平感）
- P3+ 评估

### Portfolio 与 decision 数据强耦合（被否）
- "decision confirmed → portfolio 自动开仓"会变回 paper trading
- 用户在真实账户下单 vs leek 内自动开仓的对账复杂度爆炸
- 决议保持现状：decision 与 portfolio 通过用户主动更新连接，无自动耦合

### 多账户支持（推迟）
- P1 单账户够用（"local 用户的 portfolio"）
- 多账户（如不同券商分开）是 P2 议题

## 验证标准

- 手工录入 30 个 holding，操作流畅无卡顿
- CSV 导入主流格式（5+）成功率 ≥ 95%
- Portfolio 注入 system prompt 的 token 占用控制（30 holding ≈ 300 tokens）
- agent 在 portfolio 上下文注入后能正确引用"已有持仓"做新决策（end-to-end 测试用例 ≥ 5 个）
