# P1 Spec — Tool Registry

> Agent 可调用的所有工具的完整清单与 I/O schema。包含主 agent 直接调用的工具 + subagent 可调用的工具子集。

依赖：[ADR-0010](../decisions/0010-single-agent-coordinator-subagent.md)（subagent 模型）、[`data-schema.md`](data-schema.md)（vault 写入路径）。

## 1. 设计原则

1. **结构化 I/O**：每个工具有 JSON Schema 定义的 input + output；agent 通过 LLM 的 function calling 协议调用
2. **副作用最小化**：工具明确分"读取"与"写入"；写入工具只在主 agent 可调用，subagent 永远只读
3. **超时强制**：每个工具有默认超时；超时返回 partial result + 标记 timed_out
4. **Idempotent 优先**：可能的话，重复调用同一参数返回同一结果；写入工具用 `client_request_id` 去重
5. **错误友好**：错误返回结构化（不是抛异常），agent 能根据错误信息决定重试 / 换参数 / 放弃
6. **审计完整**：每次 tool call 写 `vault.tool_call_runs`（input / output / duration / error）

## 2. 权限分层

```
                    ┌─────────────────────────────────────┐
                    │ Main Agent                          │
                    │  允许调用：所有工具                   │
                    └─────────────────────────────────────┘
                                  │
                                  │ spawn_subagent
                                  │ (with allowed_tools)
                                  ▼
                    ┌─────────────────────────────────────┐
                    │ Subagent                            │
                    │  允许调用：主 agent 给的子集          │
                    │  禁止调用：                           │
                    │   · spawn_subagent (P1 不嵌套)       │
                    │   · vault.write_*                    │
                    │   · charter.update                   │
                    │   · holdings.update                  │
                    │   · decision.confirm                 │
                    │   · review.confirm                   │
                    │   · panel.open / panel.update（间接） │
                    └─────────────────────────────────────┘
```

每个工具有 `subagent_allowed: bool` 标志，subagent runner 在调用前检查。

## 3. 工具分组

### 3.1 行情类（read-only / 主 agent + subagent）
- `quote.get` — 单标的报价
- `quote.batch` — 多标的报价
- `chart.ohlc` — K 线 / OHLCV
- `chart.intraday` — 分时
- `orderbook.snapshot` — 盘口快照（P1 nice-to-have）
- `trades.recent` — 最近成交（P1 nice-to-have）

### 3.2 资讯类
- `news.search` — 新闻搜索
- `news.fetch` — 抓取新闻全文
- `filings.list` — 上市公司公告 / 财报列表
- `filings.fetch` — 抓取公告 / 财报全文

### 3.3 财务数据类
- `financials.snapshot` — 三表 + 比率
- `financials.history` — 多年财务对比

### 3.4 技术指标类
- `indicator.compute` — 计算技术指标（MA / EMA / MACD / RSI / 布林 / KDJ ...）

### 3.5 Corpus 类
- `corpus.search` — 全文 / wikilink / 标签 / 概念图谱搜索
- `corpus.read` — 按 wikilink 或路径读取单篇
- `corpus.graph` — 获取 corpus 图结构（用于 CorpusBrain panel）

### 3.6 Vault 类（read）
- `vault.holdings.current` — 当前 portfolio 快照
- `vault.holdings.history` — portfolio 历史快照
- `vault.decisions.list` — 决策列表（含筛选）
- `vault.decisions.get` — 单个决策详情
- `vault.reviews.list` — 复盘列表
- `vault.watchlists.get` — 自选股
- `vault.charter.get` — Team Charter

### 3.7 Vault 类（write）—— 仅主 agent
- `decision.draft` — 创建 / 更新 decision draft（绑定到 task）
- `review.draft` — 创建 / 更新 review draft
- `holdings.update` — 更新 portfolio（Agent 通常不直接调，由用户 UI 触发；保留接口便于 agent 推荐用户更新）
- `panel.open` — 召唤新 panel
- `panel.update` — 更新 panel 数据
- `panel.close` — 关闭 panel
- `reasoning.add_node` — 向 ReasoningDAG 加节点（agent 主动表达推理步骤）
- `reasoning.add_edge` — 向 ReasoningDAG 加边

### 3.8 Subagent 调度
- `subagent.spawn` — 启动一个 subagent（仅主 agent）
- `subagent.list_specs` — 列出可用 subagent specs

### 3.9 通信
- `clarify.ask_user` — 主动暂停，要用户答一个问题（任务进入 awaiting_user 状态）

## 4. 通用 envelope

每个 tool 的 input 和 output 用以下统一 envelope：

```typescript
type ToolInput<T> = {
  /** 客户端去重 ID，agent 框架自动填 */
  client_request_id?: string;
  /** 超时（秒）；不填用工具默认 */
  timeout_sec?: number;
  /** 工具特定参数 */
  args: T;
};

type ToolOutput<T> = 
  | { ok: true; result: T; metadata: ToolMetadata }
  | { ok: false; error: ToolError; metadata: ToolMetadata };

type ToolMetadata = {
  duration_ms: number;
  cached: boolean;        // 是否从缓存返回
  source?: string;        // "yahoo_finance" | "tushare" | "vault" | ...
};

type ToolError = {
  code:
    | "TIMEOUT"
    | "RATE_LIMITED"
    | "NOT_FOUND"
    | "INVALID_ARGS"
    | "PERMISSION_DENIED"
    | "UPSTREAM_ERROR"
    | "UNKNOWN";
  message: string;
  retriable: boolean;
};
```

## 5. 工具详细定义

### 5.1 行情类

#### `quote.get`

```yaml
description: 获取单个标的的实时报价 + 关键指标
subagent_allowed: true
default_timeout_sec: 5
input_schema:
  type: object
  required: [ticker]
  properties:
    ticker:
      type: string
      description: 标的代码 (e.g. "NVDA", "00700.HK", "600519.SH")
    fields:
      type: array
      items:
        type: string
        enum: [price, change, change_pct, open, high, low, volume, vwap, prev_close, market_cap, pe, pb, ps, dividend_yield]
      description: 想要的字段；缺省返回所有
output_schema:
  type: object
  properties:
    ticker: string
    name: string
    exchange: string
    currency: string
    price: number
    change: number
    change_pct: number
    open: number
    high: number
    low: number
    volume: number
    market_cap: number?
    pe: number?
    ts: string  # ISO8601
```

#### `quote.batch`

```yaml
description: 批量获取多个标的报价（比循环调用 quote.get 高效）
subagent_allowed: true
default_timeout_sec: 10
input_schema:
  type: object
  required: [tickers]
  properties:
    tickers:
      type: array
      items: string
      maxItems: 100
output_schema:
  type: object
  properties:
    quotes:
      type: array
      items: <同 quote.get 的 output>
    failed:
      type: array
      items: { ticker: string, reason: string }
```

#### `chart.ohlc`

```yaml
description: 获取 OHLCV K 线
subagent_allowed: true
default_timeout_sec: 10
input_schema:
  type: object
  required: [ticker, period]
  properties:
    ticker: string
    period:
      type: string
      enum: ["1m", "5m", "15m", "30m", "1h", "1d", "1w", "1M"]
    range:
      type: string
      enum: ["1d", "5d", "1mo", "3mo", "6mo", "1y", "2y", "5y", "10y", "max"]
      default: "1y"
    adjusted:
      type: boolean
      default: true
      description: 复权
output_schema:
  type: object
  properties:
    ticker: string
    period: string
    bars:
      type: array
      items:
        type: object
        properties:
          ts: string
          open: number
          high: number
          low: number
          close: number
          volume: number
```

#### `chart.intraday`

```yaml
description: 当日分时数据
subagent_allowed: true
default_timeout_sec: 5
input_schema:
  type: object
  required: [ticker]
  properties:
    ticker: string
    granularity:
      type: string
      enum: ["1m", "5m"]
      default: "1m"
output_schema: <类似 chart.ohlc>
```

#### `orderbook.snapshot`（P1 nice-to-have）

```yaml
description: 五档盘口快照（注：A 股 / 港股 / 美股可用性不同；某些数据源不提供）
subagent_allowed: true
default_timeout_sec: 5
input_schema:
  type: object
  required: [ticker]
  properties:
    ticker: string
    levels:
      type: integer
      default: 5
      minimum: 1
      maximum: 10
output_schema:
  type: object
  properties:
    ticker: string
    bids: array of {price, qty}
    asks: array of {price, qty}
    ts: string
```

#### `trades.recent`（P1 nice-to-have）

```yaml
description: 最近 N 笔成交
subagent_allowed: true
default_timeout_sec: 5
input_schema:
  required: [ticker]
  properties:
    ticker: string
    limit: { type: integer, default: 20, maximum: 200 }
output_schema:
  trades:
    type: array
    items: { price, qty, side: "buy" | "sell", ts }
```

### 5.2 资讯类

#### `news.search`

```yaml
description: 搜索新闻（支持标题 / 内容 / 标的 / 时间窗）
subagent_allowed: true
default_timeout_sec: 15
input_schema:
  type: object
  properties:
    query: string
    tickers: array of string
    sources: array of string  # e.g. ["reuters", "bloomberg", "新华社"]
    since: string  # ISO8601
    until: string  # ISO8601
    limit: { type: integer, default: 20, maximum: 100 }
output_schema:
  type: object
  properties:
    items:
      type: array
      items:
        id: string
        title: string
        source: string
        url: string
        published_at: string
        excerpt: string
        related_tickers: array of string
```

#### `news.fetch`

```yaml
description: 抓取一篇新闻 / 文章的完整内容
subagent_allowed: true
default_timeout_sec: 20
input_schema:
  type: object
  required: [url_or_id]
  properties:
    url_or_id: string  # url 或 news.search 返回的 id
output_schema:
  type: object
  properties:
    title: string
    source: string
    url: string
    published_at: string
    content_md: string  # 转换成 markdown
    images: array of {url, caption}
```

#### `filings.list`

```yaml
description: 上市公司公告 / 财报列表
subagent_allowed: true
default_timeout_sec: 10
input_schema:
  required: [ticker]
  properties:
    ticker: string
    kind:
      type: string
      enum: ["all", "annual_report", "quarterly_report", "8k", "prospectus", "other"]
      default: "all"
    since: string
    limit: { type: integer, default: 20 }
output_schema:
  type: object
  properties:
    items:
      array of:
        id: string
        kind: string
        title: string
        filed_at: string
        url: string
```

#### `filings.fetch`

```yaml
description: 抓取一份公告 / 财报全文（支持 PDF → markdown 转换）
subagent_allowed: true
default_timeout_sec: 60
input_schema:
  required: [filing_id_or_url]
output_schema:
  content_md: string
  pages: integer
  extracted_tables: array of {caption, csv}
```

### 5.3 财务数据类

#### `financials.snapshot`

```yaml
description: 三表 + 关键比率（最新报告期）
subagent_allowed: true
default_timeout_sec: 10
input_schema:
  required: [ticker]
  properties:
    ticker: string
    period:
      type: string
      enum: ["latest_quarter", "latest_annual"]
      default: "latest_annual"
    currency: string  # ISO 4217；缺省按报表币种
output_schema:
  type: object
  properties:
    ticker: string
    period: string  # 报告期
    currency: string
    income_statement:
      revenue: number
      gross_profit: number
      operating_income: number
      net_income: number
      eps: number
    balance_sheet:
      total_assets: number
      total_liabilities: number
      total_equity: number
      cash_and_equivalents: number
      total_debt: number
    cash_flow:
      operating_cash_flow: number
      investing_cash_flow: number
      financing_cash_flow: number
      free_cash_flow: number
    ratios:
      gross_margin: number
      operating_margin: number
      net_margin: number
      roe: number
      roa: number
      debt_to_equity: number
      current_ratio: number
```

#### `financials.history`

```yaml
description: 多年财务对比（用于趋势 / YoY 计算）
subagent_allowed: true
default_timeout_sec: 15
input_schema:
  required: [ticker]
  properties:
    ticker: string
    period:
      type: string
      enum: ["annual", "quarterly"]
      default: "annual"
    n: { type: integer, default: 5, maximum: 10 }
output_schema:
  type: object
  properties:
    ticker: string
    series:
      type: array
      items: <同 financials.snapshot 的 output 加上 period>
```

### 5.4 技术指标类

#### `indicator.compute`

```yaml
description: 计算技术指标（输入价格序列，输出指标值序列）
subagent_allowed: true
default_timeout_sec: 5
input_schema:
  type: object
  required: [ticker, indicator]
  properties:
    ticker: string
    period:
      type: string
      enum: ["1m", "5m", "15m", "30m", "1h", "1d", "1w"]
      default: "1d"
    range: { type: string, default: "6mo" }
    indicator:
      type: object
      required: [name]
      properties:
        name:
          type: string
          enum: ["MA", "EMA", "MACD", "RSI", "BOLL", "KDJ", "ATR", "OBV", "CCI", "WR", "ADX"]
        params: object  # 指标特定参数（如 MA: {window: 20}）
output_schema:
  type: object
  properties:
    ticker: string
    indicator: string
    series:
      type: array
      items:
        ts: string
        values: object  # 指标特定输出（如 MACD: {dif, dea, hist}）
```

P1 起步实现：MA / EMA / MACD / RSI / BOLL（5 个最常用），其他逐步加。

### 5.5 Corpus 类

#### `corpus.search`

```yaml
description: 在 corpus 中搜索（全文 / 标签 / 概念图谱）
subagent_allowed: true
default_timeout_sec: 5
input_schema:
  type: object
  properties:
    query: string                    # 自然语言或关键词
    tags: array of string            # frontmatter tag 过滤
    cluster:                         # 限定 cluster
      type: string
      enum: ["principle", "concept", "entity", "source", "wiki"]
    limit: { type: integer, default: 10, maximum: 50 }
    include_excerpt:
      type: boolean
      default: true
output_schema:
  type: object
  properties:
    items:
      array of:
        wikilink_id: string  # e.g. "principles/margin-of-safety"
        title: string
        cluster: string
        excerpt: string  # 命中片段
        score: number
        path: string  # 相对 corpus root 的 path
```

**副作用**：每个返回的 item 触发一次 `corpus_node_activated` 事件（intensity = "search_hit"），CorpusBrain panel 收到后激活节点。

#### `corpus.read`

```yaml
description: 读取一篇 corpus 文档的完整内容
subagent_allowed: true
default_timeout_sec: 3
input_schema:
  type: object
  required: [wikilink_id_or_path]
  properties:
    wikilink_id_or_path: string
output_schema:
  type: object
  properties:
    wikilink_id: string
    title: string
    cluster: string
    frontmatter: object
    content_md: string
    related: array of {wikilink_id, title}  # 通过 wikilink 关联的其他文档
```

**副作用**：触发 `corpus_node_activated` 事件（intensity = "deep_read"）。

#### `corpus.graph`

```yaml
description: 获取 corpus 完整图结构（节点 + 边）；用于 CorpusBrain panel 启动时构图
subagent_allowed: false  # 主 agent 一般不需要直接调用，前端拉
default_timeout_sec: 3
input_schema:
  type: object
  properties:
    cluster: array of string  # 限定 cluster
output_schema:
  type: object
  properties:
    nodes:
      array of:
        id: string
        title: string
        cluster: string
        weight: number
    edges:
      array of:
        from: string
        to: string
        via: string  # "wikilink" | "tag" | "concept-link"
```

注：通常在 gateway 启动期一次性扫描 corpus 建图缓存到内存，调用 `corpus.graph` 即返回缓存内容。

### 5.6 Vault 类（read）

#### `vault.holdings.current`

```yaml
description: 当前 portfolio 持仓快照
subagent_allowed: true  # 默认允许；但主 agent 可在 spawn 时禁止以避免 anchoring
default_timeout_sec: 1
input_schema:
  type: object
  properties:
    account: { type: string, default: "main" }
output_schema:
  type: object
  properties:
    snapshot_at: string
    holdings:
      array of:
        ticker: string
        qty: number
        avg_cost: number?
        notes: string?
    summary:
      total_holdings: integer
      sector_breakdown: object  # {sector_name: pct}
      concentration_warning: string?
```

#### `vault.holdings.history`

```yaml
description: portfolio 历史快照
subagent_allowed: true
default_timeout_sec: 2
input_schema:
  properties:
    since: string
    until: string
    limit: { default: 30 }
output_schema:
  snapshots:
    array of:
      snapshot_at: string
      holdings: <同 current>
```

#### `vault.decisions.list`

```yaml
description: 列出决策（支持筛选）
subagent_allowed: true
default_timeout_sec: 2
input_schema:
  properties:
    ticker: string?
    status:
      type: string
      enum: ["any", "open", "closed", "superseded"]
      default: "any"
    since: string?
    limit: { default: 20, maximum: 100 }
output_schema:
  decisions:
    array of:
      id: string
      ticker: string
      direction: string
      size_pct: number
      stop_loss: number?
      horizon_days: integer
      status: string
      confirmed_at: string
      summary: string  # 从 rationale 提取的简短摘要
```

#### `vault.decisions.get`

```yaml
description: 单个决策的完整内容（含 rationale_md / corpus_refs）
subagent_allowed: true
default_timeout_sec: 1
input_schema:
  required: [id]
output_schema:
  <完整 decision 形态>
```

#### `vault.reviews.list`

```yaml
description: 复盘记录列表
subagent_allowed: true
default_timeout_sec: 2
input_schema:
  properties:
    decision_id: string?
    since: string?
    limit: { default: 20 }
output_schema:
  reviews:
    array of:
      id: string
      decision_id: string?
      summary_md: string
      self_score: integer
      agent_score: integer
      created_at: string
```

#### `vault.watchlists.get`

```yaml
subagent_allowed: true
default_timeout_sec: 1
input_schema:
  properties:
    id: string?  # 不填返回所有
output_schema:
  watchlists: array of {id, name, tickers, sort_order}
```

#### `vault.charter.get`

```yaml
description: 获取当前激活的 Team Charter
subagent_allowed: true
default_timeout_sec: 1
output_schema:
  <完整 charter 形态，见 interaction-model.md §6.1>
```

### 5.7 Vault 类（write）—— 仅主 agent

#### `decision.draft`

```yaml
description: 创建 / 更新一个 decision draft（绑定到当前 task）
subagent_allowed: false
default_timeout_sec: 2
input_schema:
  type: object
  required: [ticker, direction, rationale_md]
  properties:
    deliverable_id: string?  # 不填则创建新 deliverable；填则更新
    ticker: string
    direction: { enum: ["long", "short", "close", "adjust"] }
    size_shares: number?
    size_pct: number?
    stop_loss: number?
    target: number?
    horizon_days: integer?
    rationale_md: string
    corpus_refs: array of string
    review_schedule: array of string  # ISO dates
output_schema:
  type: object
  properties:
    deliverable_id: string
    mandate_check: array of {kind, severity, message}  # 实时计算的 mandate violation
    status: "draft"
```

副作用：
- 写 / 更新 `vault.deliverables`（kind=decision_draft, status=draft）
- 推 `panel.update` 事件让前端 DecisionDraft panel 刷新
- 把每个 corpus_ref 触发 `corpus_node_activated` 事件（intensity="cited"）

#### `review.draft`

```yaml
description: 创建 / 更新一个 review draft
subagent_allowed: false
default_timeout_sec: 2
input_schema:
  type: object
  required: [summary_md]
  properties:
    deliverable_id: string?
    decision_id: string?
    period_start: string?
    period_end: string?
    summary_md: string
    self_score: integer?
    agent_score: integer?
    lessons_md: string?
    corpus_inbox_candidates: array of string?  # 候选写进 corpus/inbox 的 path（P1 不实现自动写）
output_schema:
  type: object
  properties:
    deliverable_id: string
    status: "draft"
```

#### `holdings.update`

```yaml
description: 提议更新 portfolio（agent 一般不直接调；用户在 UI 操作时由 gateway 触发）
subagent_allowed: false
default_timeout_sec: 2
input_schema:
  type: object
  required: [holdings]
  properties:
    snapshot_at: string?  # 不填用 now
    holdings:
      array of:
        ticker: string
        qty: number
        avg_cost: number?
        notes: string?
        account: string?
output_schema:
  snapshot_at: string
  written: integer
```

注：agent **可以建议**用户更新（"你刚说 NVDA 加仓 15 股，要现在就更新 portfolio 吗？"），但实际写入要用户在 UI 确认或在前端调用 `holdings.update` API 直接执行。Agent 直接调这个工具的场景是 csv import 后 agent 协助补全。

#### `panel.open` / `panel.update` / `panel.close`

```yaml
# panel.open
description: 召唤一个新 panel
subagent_allowed: false
default_timeout_sec: 1
input_schema:
  required: [kind]
  properties:
    kind:
      enum: [
        "quote", "chart", "orderbook", "financial_report",
        "article", "table", "diagram",
        "reasoning_dag", "reasoning", "plan", "tool_call", "decision_draft",
        "watchlist", "portfolio", "corpus_brain"
      ]
    payload: object   # kind-specific 数据
    layout_hint:
      size: { enum: ["S", "M", "L", "XL"], default: "M" }
      position_hint: string?
output_schema:
  panel_id: string

# panel.update
description: 更新已有 panel 的数据（partial update; deep merge）
subagent_allowed: false
input_schema:
  required: [panel_id, patch]
  properties:
    panel_id: string
    patch: object  # JSON Merge Patch (RFC 7396)
output_schema:
  panel_id: string
  version: integer

# panel.close
description: 关闭 panel
subagent_allowed: false
input_schema:
  required: [panel_id]
output_schema:
  panel_id: string
```

副作用：相应推 `panel_open` / `panel_update` / `panel_close` 事件给前端。

#### `reasoning.add_node` / `reasoning.add_edge`

```yaml
# reasoning.add_node
description: 向当前 task 的 ReasoningDAG 添加一个节点
subagent_allowed: false  # subagent 通过 SubagentOutput 间接贡献，不直接调
default_timeout_sec: 1
input_schema:
  required: [kind, title]
  properties:
    kind: { enum: ["thinking", "tool_call", "observation", "corpus_ref", "decision_draft", "final_reply"] }
    title: string
    details: string?
    parent_node_id: string?  # 如果不填，连到上一个节点
    subagent_run_id: string?
output_schema:
  node_id: string

# reasoning.add_edge
description: 添加一条边（在 add_node 自动连边逻辑不够时手动加）
subagent_allowed: false
input_schema:
  required: [from, to]
output_schema:
  edge_id: string
```

注：大部分 reasoning 节点由 agent loop 自动从 LLM 流式事件生成，不需要 agent 主动调用。这两个工具用于"agent 想显式表达某个推理步骤"的场景。

### 5.8 Subagent 调度

#### `subagent.spawn`

```yaml
description: 启动一个 subagent 执行特定任务，等待返回 structured result
subagent_allowed: false  # P1 不允许 subagent 嵌套
default_timeout_sec: 90  # max_duration_sec 由 spec 决定，这是上层 wall-clock timeout
input_schema:
  required: [spec_name, scope, input]
  properties:
    spec_name:
      type: string
      enum: ["valuation_dcf", "news_summary", "ticker_research", "comparison_pair", "free_form"]
    scope:
      goal: string
      allowed_tools: array of string  # 必须是注册的工具名
      max_turns: { type: integer, default: 5, maximum: 20 }
      max_tokens: { type: integer, default: 8000, maximum: 64000 }
      max_duration_sec: { type: integer, default: 60, maximum: 300 }
      return_schema: object  # JSON Schema
    input:
      context: string
      parameters: object
output_schema:
  type: object
  properties:
    run_id: string
    success: boolean
    result: object  # 符合 scope.return_schema
    summary: string
    tokens_used: integer
    turns: integer
    duration_ms: integer
    error: string?
```

调用流程：
1. 主 agent 决定 spawn → 调 `subagent.spawn`
2. Gateway 创建 subagent runner → 写 `vault.subagent_runs`（status=running）
3. SubagentRunner 跑独立 LLM loop（独立 system prompt + 限定 tools + 预算控制）
4. 完成 / 错误 / 超时 → 更新 `vault.subagent_runs`（output / status）
5. 返回给主 agent

副作用：
- ReasoningDAG 加 subagent 分支节点
- 推 `subagent_started` / `subagent_progress` / `subagent_completed` 事件给前端

#### `subagent.list_specs`

```yaml
description: 列出当前可用的 subagent specs
subagent_allowed: false
default_timeout_sec: 1
output_schema:
  specs:
    array of:
      name: string
      description: string
      example_use_cases: array of string
      default_max_turns: integer
      return_schema: object
```

P1 提供的 subagent specs：

| spec_name | 用途 | 主要工具 |
|--|--|--|
| `valuation_dcf` | 跑一个 DCF 估值（fcf 预测、折现率、敏感度） | financials.history, indicator.compute, （未来）calculator |
| `news_summary` | 把 N 篇新闻提炼成结构化要点（情绪 / 关键事件 / 影响判断） | news.fetch |
| `ticker_research` | 全方位调研某 ticker（行情 + 财务 + 资讯 + corpus 引用） | quote, chart, financials, news, corpus.search |
| `comparison_pair` | 对比两个 ticker 的关键维度 | quote.batch, financials.snapshot, indicator.compute |
| `free_form` | 自由格式（让主 agent 在 scope.goal 里说清楚要做什么） | 主 agent 决定 allowed_tools |

每个 spec 在 `crate::subagent::specs::*` 模块定义，含独立的 system prompt template。

### 5.9 通信

#### `clarify.ask_user`

```yaml
description: 主动暂停任务，要用户回答一个问题。任务进入 awaiting_user 状态
subagent_allowed: false
default_timeout_sec: 2  # 这是 send 操作的 timeout，不是等用户答的 timeout
input_schema:
  required: [question]
  properties:
    question: string
    options: array of string?    # 多选项；不填则自由回答
    why: string?                 # 解释为什么需要问（提升用户信任）
output_schema:
  type: object
  properties:
    task_paused: boolean         # true 表示已经把 task 设成 awaiting_user
    user_response_marker: string # 等用户答时 LLM 应该停止生成；用户答完触发新一轮
```

行为：
- 推 `clarification_requested` 事件给前端 → 前端弹出问题框
- 主 agent 当前 turn 立即结束（不再 yield 更多 LLM token）
- task.status 设为 `awaiting_user`
- 用户答了之后，message 进 vault.messages → task.status 回 in_progress → agent 新一轮 LLM 调用看到这个新 message

## 6. Tool Registry 实现

```rust
// crate::tools::registry

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    fn subagent_allowed(&self) -> bool { false }
    fn default_timeout_sec(&self) -> u32 { 10 }
    async fn invoke(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError>;
}

pub struct ToolContext {
    pub user_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub invoker: Invoker,             // MainAgent | Subagent { run_id }
    pub event_bus: Arc<EventBus>,
    pub vault: Arc<Vault>,
    pub corpus: Arc<Corpus>,
    pub data_sources: Arc<DataSources>,
}

pub enum Invoker {
    MainAgent,
    Subagent { run_id: String },
}
```

调用流程（agent loop 视角）：

1. LLM 流式输出 `tool_use` block → agent loop 解析出 `name` + `args`
2. agent loop 调 `registry.dispatch(name, args, ctx)`
3. registry 检查 invoker 权限（subagent 调禁止工具 → ToolError::PermissionDenied）
4. 启动超时 / 写 `tool_call_runs`（started）
5. tool 执行 → 返回 result 或 error
6. 写 `tool_call_runs`（completed）→ 推 `tool_call_result` 事件 → 把 result 作为 ToolResult message 加进 LLM context
7. agent loop 继续下一轮 LLM 调用

## 7. 数据源后端

P1 工具需要的外部数据源：

| 数据 | 候选源 | P1 选择 |
|--|--|--|
| 美股行情 / 财务 | yahoo_finance_api / alpha_vantage / polygon | **yahoo_finance_api**（免费 + 数据全）|
| 港股 / A 股行情 | sina / netease / tushare | **新浪 / 网易非官方接口**（免费）+ tushare 作扩展 |
| 美股新闻 | newsapi.org / 自抓取 | **newsapi.org** + 关键源（reuters / bloomberg）抓取 |
| 中文财经新闻 | 雪球 / 东方财富 | **雪球**（API 友好）|
| SEC filings | SEC EDGAR | **SEC EDGAR**（免费） |
| A 股公告 | 巨潮资讯 / 东方财富 | **巨潮资讯** |

每个数据源在 `crate::data_sources::*` 模块封装。多个工具可以共享一个数据源（如 quote / chart / financials 都用 yahoo_finance_api）。

数据源失败的降级策略：
- 主源失败 → 备源（如 yahoo 挂了 → polygon）
- 所有源都失败 → 工具返回 ToolError::UpstreamError，agent 决定怎么处理

## 8. 缓存策略

行情类（高频）：
- `quote.get` / `quote.batch`：缓存 1s（实时性敏感但短期不变）
- `chart.intraday`：缓存 30s
- `chart.ohlc`（日线及以上）：缓存 5min

资讯类：
- `news.search`：缓存 1min
- `news.fetch`：缓存 1h（文章内容不变）
- `filings.fetch`：缓存 24h（公告永久不变，但偶尔修订）

财务类：
- `financials.snapshot` / `financials.history`：缓存 24h（财报每季度更新）

Corpus 类：
- 启动期建好的图缓存常驻内存，文件改动 → 重建（P1 简化：手动 reload）
- `corpus.read` 缓存 5min

实现：用 `moka` crate（async LRU cache）。

## 9. 安全 / 限流

### 9.1 速率限制
- 每个数据源对 LLM token 的发请求频率限制（防止打挂）
- 每个用户对每个 tool 的速率限制（防止 agent 失控调用导致 quota 燃烧）

### 9.2 输入校验
- 每个 tool 的 input 必须通过 JSON Schema validation
- ticker 格式校验（防止 injection）
- URL 白名单（news.fetch 只允许已知新闻源 + corpus 内 URL）

### 9.3 输出 sanitization
- markdown 内容防 XSS（前端 sanitize；后端不 trust）

## 10. P1 实施 checklist

- [ ] `Tool` trait + `ToolRegistry`
- [ ] `ToolContext` + `Invoker` 权限检查
- [ ] 行情类（quote / chart）实现 + yahoo_finance_api 集成
- [ ] 财务类（financials）实现
- [ ] 资讯类（news / filings）实现 + newsapi / SEC EDGAR 集成
- [ ] 技术指标类（5 个起步：MA / EMA / MACD / RSI / BOLL）
- [ ] Corpus 类（search / read / graph）实现
- [ ] Vault read 类（holdings / decisions / reviews / watchlists / charter）实现
- [ ] Vault write 类（decision.draft / review.draft / panel.* / reasoning.*）实现
- [ ] subagent.spawn + 5 个 specs（valuation_dcf / news_summary / ticker_research / comparison_pair / free_form）
- [ ] clarify.ask_user 实现 + task awaiting_user 状态机
- [ ] 缓存层（moka）
- [ ] 单元测试 + 集成测试（每个 tool 至少 3 个 case）
- [ ] e2e 测试：完整 task 调用 5+ 工具的场景
