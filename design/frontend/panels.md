# L.E.E.K Frontend Panels

> chat-canvas 中所有 panel 类型的完整定义。本文档是 UX 设计师和前端开发者的工作清单——读完它你应该知道每个 panel 是什么、长什么样、什么时候出现、怎么交互。

先读 [`concept.md`](concept.md) 再读这份。

## 1. Panel 概念

### 1.1 什么是 Panel

**Panel = 一个 typed artifact（warp 风格）**——agent 工作时召唤的、有结构、有状态、可交互的工作面元素。

每个 panel：
- 有明确的 `kind`（类型）
- 有明确的 `data shape`（数据 schema）
- 有明确的 `state machine`（生命周期状态）
- 有明确的 `event subscriptions`（订阅哪些 gateway 事件）
- 有明确的 `render strategy`（DOM / Canvas / 混合）

Panel **不是**：
- 不是简单的"内嵌图片"
- 不是固定 tab（用户不在固定 7 个 tab 里点选）
- 不是普通 component（每个 panel 是 typed + persisted artifact，存进 vault）

### 1.2 Panel 的生命周期

```
                  agent 决定召唤
                        │
                        ▼
                ┌──────────────┐
                │  opening     │  agent 推 panel_open 事件，前端创建空 panel
                └───────┬──────┘
                        │ payload 准备好
                        ▼
                ┌──────────────┐
                │  loading     │  显示 skeleton / 数据拉取中
                └───────┬──────┘
                        │ 数据到位
                        ▼
                ┌──────────────┐
                │  ready       │  正常显示
                └───────┬──────┘
                        │
                        ├─── update events ─→ ready (重渲染)
                        ├─── interrupt ─→ paused
                        └─── close → closed (持久化进 vault.artifacts)
```

每个 panel 都有 `panel_id`（UUID），全局唯一。session 重新打开时，从 vault 加载所有 panel state 重建 canvas。

### 1.3 Panel 的布局规则

- **默认 layout**：自动网格（基于 panel 大小标签：S / M / L / XL）
- **用户可自由调整**：拖动 / 缩放 / 钉住 / 最小化 / 关闭
- **持久化**：layout 状态 per-session 存 `vault.artifacts.layout`（kind=session_layout）
- **特殊 panel**：CorpusBrain 默认是侧栏 ambient 视图（持续可见、可最小化但不会被关闭）

### 1.4 Panel 的事件订阅

每个 panel 类型订阅一组 gateway 事件 kind：

```
PanelType        → 订阅的 events
─────────────────────────────────────────────────
Quote            → tick.<ticker>
Chart            → ohlc.<ticker>.<period>
ReasoningDAG     → reasoning_dag_node, reasoning_dag_edge
CorpusBrain      → corpus_node_activated
ToolCall         → tool_call_start, tool_call_args_delta, tool_call_result
DecisionDraft    → decision_draft_*
Portfolio        → portfolio_snapshot_updated
... (详见每类 panel 章节)
```

事件的具体 schema 在 `p1-spec/api.md` 里定义。

## 2. Panel 类型清单（P1）

按用途分组：

| 组 | Panel | 用途 | 渲染 |
|--|--|--|--|
| **核心叙事** | CorpusBrain | corpus 知识图谱 + 激活动效 | Canvas / WebGL |
| **核心叙事** | ReasoningDAG | 当前任务的推理 DAG + 动效 | Canvas + DOM 混合 |
| **数据可视化** | Quote | 单标的快照 + tick stream | DOM (高频字段 ref) |
| **数据可视化** | Chart | K 线 / 分时（Candle / Line / Area）+ 指标 | Canvas (lightweight-charts) |
| **数据可视化** | OrderBook | 盘口 + 成交流 | Canvas (自写) |
| **数据可视化** | FinancialReport | 财务三表 + 多年对比 | DOM (table) |
| **数据可视化** | Heatmap | 行业 / 板块涨跌热图（P2 候选）| Canvas |
| **数据可视化** | CorrelationMatrix | 相关性矩阵（P2 候选）| Canvas |
| **文本 / 文档** | Article | 新闻 / 公告 / 研报 | DOM (markdown 渲染) |
| **文本 / 文档** | Table | 通用表格 | DOM (TanStack Table) |
| **文本 / 文档** | Diagram | SVG / Mermaid（产业链 / 估值因子） | SVG |
| **Agent 状态** | Reasoning | 思考过程 + thinking traces 展开 | DOM |
| **Agent 状态** | Plan | 任务计划（typed plan） | DOM |
| **Agent 状态** | ToolCall | 工具调用实时进度 | DOM |
| **Agent 状态** | DecisionDraft | 决策草稿等待 confirm | DOM (form) |
| **操作 / 持续视图** | WatchList | 自选股 | DOM (table 简化) |
| **操作 / 持续视图** | Portfolio | 持仓视图（投研参考） | DOM (table + 高频价格) |

**P1 必做**：CorpusBrain、ReasoningDAG、Quote、Chart、Article、Table、Reasoning、Plan、ToolCall、DecisionDraft、WatchList、Portfolio（12 类）。

**P1 nice-to-have**：OrderBook、FinancialReport、Diagram（3 类，看排期）。

**P2 候选**：Heatmap、CorrelationMatrix。

下面逐类展开。

---

## 3. CorpusBrain ⭐ — 核心叙事 panel

**这是 L.E.E.K 的产品 signature。投入最多 craft 在这里。**

### 3.1 用途

可视化 corpus 知识图谱，并在 agent 思考过程中**实时展示哪些"神经元"被激活**——把抽象的"AI 在思考"变成可见的过程，把用户自己策划的智慧库可视化为大脑。

### 3.2 视觉

```
┌─────────────────────────────────────────────────────────────────┐
│ CorpusBrain          [全屏] [搜索] [筛选: principles ▾] [─] [×]│
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│              ●                                                  │
│             ╱│╲                  ●─────● 激活脉冲              │
│            ● │ ●                ╱│╲    │                       │
│            │ │ │               ● │ ●   ●                       │
│            ● ● ●               │ │ │   │                       │
│              │                 ● ● ●   ●                       │
│              ●                                                  │
│                              currently activated:               │
│                              · margin-of-safety                 │
│                              · owners-earnings                  │
│                              · circle-of-competence             │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 3.3 数据形态

```typescript
type CorpusGraph = {
  nodes: Array<{
    id: string;                   // wikilink ID, e.g. "principles/margin-of-safety"
    title: string;                // "Margin of Safety"
    kind: "principle" | "concept" | "entity" | "source" | "wiki";
    cluster?: string;             // 用于分组着色：principles | knowledge | sources
    weight?: number;              // 视觉权重（按引用度计算，影响节点大小）
  }>;
  edges: Array<{
    from: string;
    to: string;
    via: "wikilink" | "tag" | "concept-link";
  }>;
};

type ActivationEvent = {
  node_id: string;
  intensity: "search_hit" | "deep_read" | "cited";
  ts: string;
  session_id: string;
};
```

### 3.4 渲染策略

**Canvas + WebGL**（用 PixiJS 或自写 WebGL）。

理由：
- 节点数量 100-500，DOM 渲染会卡
- 力导布局每帧需要更新所有节点位置
- 动效（脉冲、ripple、扩散）在 Canvas 上更可控

候选库：
- **PixiJS**（WebGL，性能好，社区成熟，最推荐）
- **Cytoscape.js**（图特化，但默认 SVG，性能略差；Canvas 渲染需 plugin）
- **D3-force + Canvas**（最灵活但工作量大）
- **Sigma.js**（图特化，WebGL，但生态不如 PixiJS）

**P1 推荐 PixiJS**。

### 3.5 力导布局 + cluster

- 物理模拟：节点间斥力 + 边弹簧吸引 + 中心向心力
- Cluster 着色：principles 一种主色（如 amber）、concepts 一种（如 cyan）、entities 一种（如 magenta）、sources 一种（如 dim gray）
- 节点大小：基于被引用次数（权重）
- 边粗细：基于引用关系强度（多种关系 → 粗）

启动时跑模拟到稳定状态（前 200 帧），之后冻结大致位置（用户拖动后才变化）。

### 3.6 激活动效（核心 craft）

激活分 3 强度，对应不同视觉：

| 强度 | 触发 | 视觉 |
|--|--|--|
| **search_hit** | agent 调 corpus_search 命中 | 节点单次脉冲（150ms 峰值，500ms 衰减），无 ripple |
| **deep_read** | agent 调 corpus_read 全文读取 | 节点持续亮 1s，颜色饱和度提升 |
| **cited** | agent 在 decision/review 的 corpus_refs 写入 | 节点变核心色 + 边缘扩散 ripple（500ms）+ 与已激活节点之间的边浮现 |

事件驱动：每个 ActivationEvent 到达 → 触发一次动效。多事件同时到达时排队（300ms 内最多并发 3 个）。

**视觉示意**：

```
        激活时:                       cited 时:
        
          ●                              ●  ←── 高亮核心色
        ╱┃╲                            ╱┃╲
       ● ┃ ●  ←── 旁边节点未激活      ● ┃ ●
         ┃                              ┃ + ripple 边缘扩散
         ●                              ●
                                        ┃ ←── 边浮现
                                        ●  ←── 已激活的另一节点
```

### 3.7 交互

- **左键拖动节点** → 用户调整位置（持久化）
- **左键点击节点** → 弹出节点详情（小 popover：title + cluster + 当前 session 激活次数 + "查看全文"按钮）
- **"查看全文"** → 召唤一个 Article panel 显示 corpus 文件内容
- **滚轮** → 缩放
- **空白拖动** → 平移
- **shift + 选择** → 多选高亮
- **搜索框**：输入关键字 → 匹配的节点高亮 + 自动定位
- **筛选 dropdown**：只显示某 cluster（principles / concepts / entities / sources）
- **全屏按钮** → CorpusBrain 接管整个 canvas（chat 主轴变窄）
- **激活路径回放**（P1.5）：点击某个 session 的回放按钮 → 重放该 session 中所有激活事件按时间序

### 3.8 与 ReasoningDAG 联动

- 点击 ReasoningDAG 中的某节点（如 "我引用了 margin-of-safety"）→ CorpusBrain 中对应 corpus 节点高亮 + 镜头平移过去
- 点击 CorpusBrain 节点 → 如果 ReasoningDAG 当前 session 中引用过该节点，那个 reasoning 节点高亮

### 3.9 持久化

- 当前 session 的所有激活事件存 `vault.artifacts (kind=corpus_activation_trace)`
- 节点位置（用户拖动调整后）存 `vault.artifacts (kind=corpus_layout)` per-user
- corpus graph 本身**不存 vault**——启动期从 corpus 文件系统重新构建

### 3.10 数据来源

启动期 gateway 跑一次 corpus 扫描（所有 .md 文件 + frontmatter + wikilink 解析）→ 生成 graph → embed 在内存 → 通过新增 tool `corpus_graph` 给前端拉取。

corpus 文件变化时（手工编辑），需要重新扫描 → 推一个 `corpus_graph_changed` 事件触发前端重建。P1 简化为"启动期一次 + 手动 reload 按钮"，不做 file watcher。

---

## 4. ReasoningDAG ⭐ — 核心叙事 panel

CorpusBrain 的**孪生 panel**——展示 agent 当前任务的推理过程。

### 4.1 用途

把 agent 思考过程可视化为 DAG（有向无环图），节点 = 推理步骤 / tool call / 观察 / 引用，边 = 因果连接。流式实时展开，看着 agent "想出来"。

### 4.2 视觉

```
┌─────────────────────────────────────────────────────────────────┐
│ ReasoningDAG                                       [─] [×]     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   ◆ 用户：NVDA 加仓?                                            │
│   │                                                             │
│   ●─→ ToolCall: portfolio                                       │
│   │   ▼                                                         │
│   ●─→ Observation: 已有 50 股                                   │
│   │                                                             │
│   ●─→ ToolCall: quote NVDA                                      │
│   │   ▼                                                         │
│   ●─→ Observation: $480, ↑ 3M                                   │
│   │                                                             │
│   ●─→ Corpus: margin-of-safety 🧠 ←── 同时激活 Brain            │
│   │                                                             │
│   ●─→ Reasoning: 估值偏高，但护城河...                          │
│   │                                                             │
│   ◆ 决策草稿: +15 股 stop $440                                  │
│                                                                 │
│        ┃ traveling pulse 沿边动效                               │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

节点形状区分类型：
- ◆ 入口（用户问题）/ 出口（决策草稿 / 答复）
- ● 中间推理 / observation
- ▶ ToolCall
- 🧠 Corpus 引用（激活联动 CorpusBrain）

### 4.3 数据形态

```typescript
type ReasoningNode = {
  id: string;
  kind: "user_input" | "thinking" | "tool_call" | "observation"
      | "corpus_ref" | "decision_draft" | "final_reply";
  title: string;
  details?: string;
  ts: string;
  status: "active" | "completed" | "errored";
};

type ReasoningEdge = {
  from: string;
  to: string;
};
```

### 4.4 渲染策略

DOM + SVG（节点用 DOM box 显示文本，边用 SVG path）。

理由：节点数量较少（典型 10-50），节点内容是文本（DOM 友好），交互（点击 / hover）DOM 易实现。

### 4.5 流式展开 + 动效

- 节点逐个出现：node 入场动效（fade + scale 150ms）
- 边逐条出现：edge stroke draw 动效（200ms）
- 当前活跃节点：脉冲 outline（持续直到下一节点接力）
- 边的 traveling pulse：沿边路径走光点动效（200ms / 边）

### 4.6 交互

- 点击节点 → 弹出 details（thinking 全文 / tool 参数 / observation 详细 / corpus 引用 popover）
- 点击 corpus_ref 节点 → CorpusBrain 同步高亮
- hover 节点 → 高亮所有上下游节点（path 高亮）
- 长任务可折叠：点击某个 thinking 节点 → 展开它的 sub-DAG（agent 嵌套调用产生的 sub-reasoning）
- "重放" 按钮：回到任务开始，再放一遍动效

### 4.7 持久化

- 整个 DAG（节点 + 边 + 时序）存 `vault.artifacts (kind=reasoning_dag)`
- 用户后来打开同一 session 重看时，能完整重建（含动效回放选项）

---

## 5. Quote — 单标的快照

### 5.1 用途

显示一个 ticker 的实时报价 + 短期走势 + 关键指标。

### 5.2 视觉

```
┌──────────────────────────────────┐
│ NVDA  Nvidia Corp     [─] [×]    │
├──────────────────────────────────┤
│   $480.20    ↑ +9.85 (+2.10%)   │
│                                  │
│   ▁▂▂▃▄▆█▇▆▆▅▆▇█  (sparkline)  │
│                                  │
│   开 472.00  高 482.50  低 470.10│
│   量 35.2M   涨幅 2.10%         │
│   PE 65.4   市值 12T            │
└──────────────────────────────────┘
```

### 5.3 数据形态

```typescript
type QuoteData = {
  ticker: string;
  name: string;
  price: number;
  change: number;
  change_pct: number;
  open: number;
  high: number;
  low: number;
  volume: number;
  pe?: number;
  market_cap?: number;
  ts: string;
};
```

### 5.4 渲染策略

**DOM**——但高频更新字段（price / change / change_pct）通过 ref 直接更新 textContent，不进 Solid 的 reactivity（虽然 Solid 也能处理，但 ref 更省事且能轻松加 flash 动效）。

Sparkline 用 mini canvas 或 SVG path（更新频率低，DOM/SVG 都可）。

### 5.5 实时性

- 订阅 `tick.<ticker>` 事件
- price 更新时：textContent 直接刷新 + 短暂 flash（绿/红方向，100ms）
- 其他字段更新频率低（K 线汇总），用 Solid signal

### 5.6 交互

- 点击 → 召唤 Chart panel（同 ticker、默认日线）
- 右键 → 添加到 watchlist / 设置提醒（P2）

---

## 6. Chart — K 线 / 分时

### 6.1 用途

K 线图、分时图、技术指标 overlay。

### 6.2 视觉

```
┌─────────────────────────────────────────────────────────────────┐
│ NVDA  [1m] [5m] [15m] [1H] [D] [W]  [指标 +] [─] [×]           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│         ┃           ╲    ╱    ╲    ↗  ↗      MA20             │
│      ╱╲┃   ┃  ╱╲   ╲╲  ╱╱      ╲↗↗╱                          │
│     ╱  ╲┃  ┃ ╱  ╲   ╲╲╱        ╱   ↘                          │
│    ┃    ╲  ╲ ╲   ╲  ┃                ╲                         │
│   ┃           ┃                                                 │
│  ─────────────────────────────────────────────                  │
│   volume: ▁▂▃▅▆▇█▆▅▃▂▁▂▃▄▅▆▇█▅▃                               │
└─────────────────────────────────────────────────────────────────┘
```

### 6.3 数据形态

OHLCV + 指标参数。

### 6.4 渲染策略

**Canvas (lightweight-charts)**——TradingView 出品，性能极好，覆盖 P1 所有需求。

候选 fallback：ECharts（如果 lightweight-charts 缺某指标）。

### 6.5 交互

- 周期切换：1m / 5m / 15m / 1H / D / W / M
- 指标添加：MA / EMA / MACD / RSI / 布林 / KDJ 等（弹窗选）
- 鼠标 crosshair：显示精确价格 + 时间
- 缩放 / 平移：滚轮 / 拖动
- 区间选择 → 召唤"区间统计" mini panel（P2）

---

## 7. OrderBook — 盘口

### 7.1 用途

显示买卖五档 / 十档 + 实时成交流。

### 7.2 视觉

```
┌────────────────────────────┐
│ NVDA Order Book   [─] [×]  │
├────────────────────────────┤
│  Ask                       │
│  481.00  ████ 1,200        │
│  480.80  ██ 800            │
│  480.50  █ 500             │
│  ── Spread $0.30 ──        │
│  480.20  ██ 900            │
│  480.00  ██████ 2,100      │
│  479.80  ████ 1,500        │
│                            │
│  Trades                    │
│  480.20  100  Buy   14:22  │
│  480.20  500  Sell  14:22  │
│  480.30  200  Buy   14:21  │
└────────────────────────────┘
```

### 7.3 渲染策略

**Canvas（自写）**——实时 tick 流场景，DOM 性能不够。

P1 简化版：DOM + ref 高频字段更新（如果 tick 频率不高，可以先 DOM 起步）。

### 7.4 数据形态

```typescript
type OrderBookData = {
  ticker: string;
  bids: Array<{ price: number; qty: number }>;
  asks: Array<{ price: number; qty: number }>;
  trades: Array<{ price: number; qty: number; side: "buy" | "sell"; ts: string }>;
};
```

### 7.5 P1 范围

- 五档（不做十档+）
- 简化的 trades stream（最近 20 笔）
- 不做 depth chart（P2）

OrderBook 在 P1 标记为 **nice-to-have**——优先级低于 Quote / Chart。如果 schedule 紧可以推迟到 P1.5。

---

## 8. FinancialReport — 财务三表

### 8.1 用途

展示标的的资产负债表 / 利润表 / 现金流量表，多年对比。

### 8.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ NVDA Financials  [BS] [IS] [CF]      [Year ▾] [─] [×]  │
├─────────────────────────────────────────────────────────┤
│                  2025      2024      2023      YoY      │
│  Revenue        130.5B    96.7B     60.9B    +35%      │
│  Gross Profit   100.4B    71.4B     34.4B                │
│  Operating Inc   80.1B    49.5B     19.6B                │
│  Net Income      72.6B    42.1B     14.6B                │
│  EPS              2.97     1.71      0.57                │
│                                                         │
│  Margins                                                │
│  Gross %         77.0%    73.8%     56.5%                │
│  Operating %     61.4%    51.2%     32.1%                │
│  Net %           55.6%    43.5%     24.0%                │
│                                                         │
│  ▁▃▆█  Revenue trend                                    │
└─────────────────────────────────────────────────────────┘
```

### 8.3 渲染

DOM table。可以用 TanStack Table 处理排序 / 高亮 / 同比计算。

### 8.4 交互

- 切换三表 tab
- 切换年度（5 年 / 10 年）
- 同比 / 环比切换
- 点击数字 → 弹出趋势 sparkline
- "对比模式" → 拖入另一 ticker 同时显示（P1.5）

P1 标记为 **nice-to-have**。

---

## 9. Article — 新闻 / 公告 / 研报

### 9.1 用途

显示一篇文章 / 公告 / 研报内容。

### 9.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ Article                                       [─] [×]  │
├─────────────────────────────────────────────────────────┤
│ NVDA 财报暴击预期，云收入同比 +120%                      │
│ Reuters · 2026-04-30 · 3 min read                       │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  Nvidia 周三公布的季度财报显示，数据中心业务收入达       │
│  到 350 亿美元，同比增长 120%...                         │
│                                                         │
│  [图表/数据嵌入]                                         │
│                                                         │
│  完整阅读 ↗  原文链接                                    │
└─────────────────────────────────────────────────────────┘
```

### 9.3 数据形态

```typescript
type ArticleData = {
  title: string;
  source: string;
  url?: string;
  published_at: string;
  content_md: string;       // markdown
  excerpt?: string;
  related_tickers?: string[];
  related_corpus_refs?: string[];
};
```

### 9.4 渲染

DOM + markdown renderer（如 `solid-markdown` 或调 marked.js）。

### 9.5 交互

- 滚动阅读
- 高亮选中文字 → "用这段问 agent" / "标记为重要"
- 相关标的 → 召唤 Quote panel
- 相关 corpus 引用 → 同时激活 CorpusBrain

---

## 10. Table — 通用表格

### 10.1 用途

筛选结果 / 排名 / 对比 / 任意结构化数据。

### 10.2 视觉

```
┌────────────────────────────────────────────────────────────────┐
│ Top Tech P/E < 30 (2026 May)                       [─] [×]    │
├────────────────────────────────────────────────────────────────┤
│  Ticker  Name      Price    PE    Mkt Cap   Rev YoY  ←sortable│
│  GOOGL   Alphabet  $185    25.3   2.3T      +14%             │
│  META    Meta      $570    28.1   1.5T      +21%             │
│  ...                                                           │
└────────────────────────────────────────────────────────────────┘
```

### 10.3 渲染

DOM + TanStack Table（跨框架，Solid 适配可用）。

### 10.4 交互

- 排序 / 筛选 / 列显示选择
- 点击 ticker → 召唤 Quote panel
- 拖拽行 → 添加到 watchlist

---

## 11. Diagram — SVG / Mermaid

### 11.1 用途

产业链图、估值因子分解、概念关系图、流程图。区别于 ReasoningDAG（reasoning DAG 是动态的、属于当前任务）和 CorpusBrain（corpus 大脑视图），Diagram 是 agent 输出的静态可视化。

### 11.2 视觉

```
┌──────────────────────────────────────────────────────────┐
│ NVDA 产业链                                  [─] [×]    │
├──────────────────────────────────────────────────────────┤
│                                                          │
│   TSMC ──→ NVDA ──→ Microsoft / Meta / Google           │
│             │                                            │
│             ↓                                            │
│        AI Inference / Training Workloads                │
│             │                                            │
│             ↓                                            │
│        Enterprise / Hyperscaler Demand                  │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

### 11.3 渲染

SVG + Mermaid（agent 输出 mermaid 字符串，前端渲染）。

P1 标记为 **nice-to-have**。

---

## 12. Reasoning — 思考过程展开

### 12.1 用途

折叠 / 展开式的"agent 在想什么"——thinking traces 的纯文本形式。

不同于 ReasoningDAG（DAG 形式 + 动效），Reasoning panel 是**纯文本的 thinking 流**，适合用户想"读完整思路"的场景。

### 12.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ Reasoning  [折叠 ▾]                          [─] [×]    │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  让我先看看用户当前的持仓。我看到他有 50 股 NVDA，     │
│  这意味着 NVDA 已经是他的核心仓位之一。                  │
│                                                         │
│  接下来要考虑：                                          │
│  1. 这只股票的当前估值是否还在 margin-of-safety 范围内  │
│  2. 加仓会不会让单标位置超过 mandate 上限                │
│  3. 他的整体 portfolio 集中度怎么样                      │
│                                                         │
│  让我查一下当前价格和最近的财报...                       │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

### 12.3 渲染

DOM。流式接收 LLM thinking delta → 逐字 / 逐 token 显示。

### 12.4 交互

- 折叠 / 展开
- 复制全文
- 标记片段为 important（影响 corpus 候选 / review 引用）

---

## 13. Plan — 任务计划

### 13.1 用途

agent 在长任务中先列出"我准备做什么"——typed plan，用户可以看 / 干预。

灵感来自 warp 的 typed plan。

### 13.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ Plan: 分析 NVDA 加仓决策                       [─] [×]  │
├─────────────────────────────────────────────────────────┤
│  ☑ 1. 查询当前 portfolio                                │
│  ☑ 2. 拉 NVDA 实时行情 + 近期走势                       │
│  ⏵ 3. 翻 corpus: margin-of-safety, kelly-criterion      │
│  □ 4. 计算建议加仓量                                     │
│  □ 5. 生成 decision draft                                │
│                                                         │
│  [跳过当前 step] [提前停止]                              │
└─────────────────────────────────────────────────────────┘
```

### 13.3 渲染

DOM。

### 13.4 交互

- 用户可以"跳过当前步骤"或"提前停止"——发送 control 指令到 gateway
- agent 在执行时实时 check 步骤状态

P1 简化版：只显示 plan，不允许用户干预（干预 P1.5）。

---

## 14. ToolCall — 工具调用进度

### 14.1 用途

实时显示 agent 调用某个工具的过程。

### 14.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ Tool: corpus_search                          ⟳ running  │
├─────────────────────────────────────────────────────────┤
│  query: "margin of safety NVDA chip valuation"          │
│  →                                                      │
│  · principles/margin-of-safety.md ✓                    │
│  · concepts/owners-earnings.md ✓                       │
│  · entities/buffett.md ✓                               │
│  · ... (5 more)                                         │
│                                                         │
│  duration: 230ms · 8 hits                               │
└─────────────────────────────────────────────────────────┘
```

### 14.3 状态机

```
queued → running (动效: spinner) → result (固定结果显示) | error
```

### 14.4 渲染

DOM。

### 14.5 交互

- 点击 hit → 召唤 Article / corpus 节点联动
- 复制工具参数 / 结果（debug 用）
- 折叠（默认完成后自动折叠）

---

## 15. DecisionDraft — 决策草稿

### 15.1 用途

agent 生成的投资决策草稿，等用户编辑 / confirm。

### 15.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ Decision Draft                              [─] [×]     │
├─────────────────────────────────────────────────────────┤
│  Ticker:       [ NVDA ▾]                                │
│  Direction:    ◉ Long  ○ Short  ○ Close  ○ Adjust      │
│  Size:         [ 15 ] shares  ([1.5%] of portfolio)     │
│  Stop Loss:    [ $440.00 ]                              │
│  Target:       [ $560.00 ] (optional)                   │
│  Horizon:      [ 90 ] days                              │
│  Review at:    [ 2026-06-01 ] [ 2026-08-01 ]   [+]     │
│                                                         │
│  Rationale:                                             │
│  ┌─────────────────────────────────────────────────┐   │
│  │ 当前估值偏高（PE 65 vs 行业 30），但...           │   │
│  │ - 数据中心收入同比 +120%, 拐点确认              │   │
│  │ - 护城河：CUDA 生态 + ...                       │   │
│  │ - margin-of-safety: 当前价距 DCF intrinsic 12% │   │
│  │ ...                                             │   │
│  └─────────────────────────────────────────────────┘   │
│                                                         │
│  Cited (corpus):                                        │
│  · margin-of-safety  · kelly-criterion                  │
│  · owners-earnings  · circle-of-competence              │
│  ┌────────────────────────────────────────────────┐    │
│  │ Mandate check: ✓ size 1.5% < 10% 上限          │    │
│  │ Mandate check: ⚠️  集中度警告：科技股已 65%     │    │
│  └────────────────────────────────────────────────┘    │
│                                                         │
│  [Discard]  [Save as Draft]  [Confirm ✓]                │
└─────────────────────────────────────────────────────────┘
```

### 15.3 数据形态

```typescript
type DecisionDraft = {
  id: string;
  session_id: string;
  ticker: string;
  direction: "long" | "short" | "close" | "adjust";
  size_shares?: number;
  size_pct?: number;
  stop_loss?: number;
  target?: number;
  horizon_days: number;
  review_schedule: string[];      // ISO dates
  rationale: string;              // markdown
  corpus_refs: string[];
  mandate_violations: Array<{ kind: string; severity: "warn" | "block"; message: string }>;
  status: "draft" | "confirmed" | "discarded";
};
```

### 15.4 渲染

DOM (form)。

### 15.5 交互

- 行内编辑：所有字段可编辑（agent 给的是建议，用户可调整）
- Mandate check 实时计算（编辑某字段后立即重算）
- 引用的 corpus 标签 → 点击同时激活 CorpusBrain
- Confirm → 写入 vault.decisions, status=confirmed
- Discard → 写入 vault.decisions, status=discarded（保留以便复盘）
- Save as Draft → 写入 vault.decisions, status=draft（可后续继续编辑）

---

## 16. WatchList — 自选股

### 16.1 用途

用户的自选股列表，常驻显示。

### 16.2 视觉

```
┌─────────────────────────────────────────────────────────┐
│ Watchlist: Tech AI                            [⚙] [─]  │
├─────────────────────────────────────────────────────────┤
│  NVDA   $480.20   ↑ +2.10%   PE 65.4                   │
│  GOOGL  $185.50   ↑ +1.20%   PE 25.3                   │
│  META   $570.10   ↓ -0.80%   PE 28.1                   │
│  MSFT   $420.30   ↑ +0.50%   PE 35.2                   │
│  TSLA   $230.40   ↓ -2.10%   PE 80.5                   │
│                                                         │
│  [+ Add Ticker]                                         │
└─────────────────────────────────────────────────────────┘
```

### 16.3 数据形态

```typescript
type WatchlistData = {
  id: string;
  name: string;
  tickers: string[];
  // 价格数据按 ticker 订阅 tick.<ticker> 实时刷新
};
```

### 16.4 渲染

DOM (mini table)，价格字段通过 ref 高频更新。

### 16.5 交互

- 拖入 chat 输入框 → 插入 mention chip
- 点击 ticker → 召唤 Quote / Chart
- 编辑 watchlist：添加 / 删除 / 重命名 / 多个 watchlist 切换
- 排序：按代码 / 涨幅 / 名字

---

## 17. Portfolio — 持仓视图（投研参考）

### 17.1 用途

用户当前真实持仓的镜像，agent 把它当 context 做决策。**不是模拟交易状态**（详见 [ADR-0009](../decisions/0009-portfolio-as-research-context.md)）。

### 17.2 视觉

```
┌─────────────────────────────────────────────────────────────┐
│ Portfolio  [Snapshot: 2026-05-01 ▾]   [Import CSV]   [─]   │
├─────────────────────────────────────────────────────────────┤
│  Ticker  Qty    Avg Cost   Now    P&L          % of Total  │
│  NVDA    50     $420       $480   +$3,000  +14.3%   25%    │
│  AAPL    200    $145       $190   +$9,000  +15.5%   30%    │
│  GOOGL   100    $130       $185   +$5,500  +42.3%   18%    │
│  ...                                                        │
│                                                             │
│  Total: 5 holdings   Market Value: $124,800                │
│  Sector breakdown: Tech 75%, Consumer 15%, ...             │
│                                                             │
│  [Edit] [Add Holding] [Export]                              │
└─────────────────────────────────────────────────────────────┘
```

### 17.3 数据形态

参考 [ADR-0002](../decisions/0002-sqlite-vault-single-db.md) 的 `holdings` 表。

### 17.4 渲染

DOM (table)。当前价字段 ref 高频更新。

### 17.5 交互

- 编辑：行内编辑 qty / avg_cost / notes
- 添加 / 删除 holding
- Snapshot 切换：dropdown 选历史时刻
- CSV 导入 / 导出
- 集中度警告：sector / 单标超过 mandate 上限时高亮
- 拖入 chat → 自动插入"我的 [ticker] 持仓 [qty] 股"context

### 17.6 与 agent 的关联

最新 snapshot 的所有 holdings 自动注入 agent system prompt。Portfolio panel 与 agent 不双向耦合——agent 不能改 portfolio。

---

## 18. Heatmap / CorrelationMatrix — P2 候选

### 18.1 Heatmap

行业 / 板块涨跌热图，类似 finviz.com 风格。

### 18.2 CorrelationMatrix

多标的相关性矩阵，agent 在做组合分析时召唤。

P2 候选，**P1 不做**。设计稿可以预留视觉但不实施。

---

## 19. Panel 间交互模式

### 19.1 召唤模式

agent 通过推送 `panel_open` 事件召唤 panel。前端按事件 payload 创建 panel，初始化数据，订阅相关事件流。

### 19.2 联动机制

- **CorpusBrain ⟷ ReasoningDAG**：corpus_ref 节点点击双向高亮
- **Quote ⟷ Chart**：点击 Quote 召唤同 ticker 的 Chart
- **Article ⟷ CorpusBrain**：Article 中的相关 corpus refs 点击激活 Brain
- **Watchlist / Portfolio ⟷ Quote**：点击 ticker 行召唤 Quote

### 19.3 mention chip

chat 输入框支持：

- `@NVDA` → ticker chip（绿色，可拖动）
- `@corpus:margin-of-safety` → corpus chip（紫色）
- `@decision:abc123` → decision chip（橙色，引用历史决策）
- `@portfolio:current` → portfolio chip（蓝色，引用当前持仓）

mention chip 在提交时被解析成 agent 的 strong context（不是普通文本，而是 typed reference）。

## 20. Panel 总数与 P1 排期

P1 必做（12 类）：CorpusBrain、ReasoningDAG、Quote、Chart、Article、Table、Reasoning、Plan、ToolCall、DecisionDraft、WatchList、Portfolio

P1 nice-to-have（3 类）：OrderBook、FinancialReport、Diagram

P2 候选（2 类）：Heatmap、CorrelationMatrix

UX 设计排期建议：

1. **第一波 craft**：CorpusBrain + ReasoningDAG（产品 signature，最多打磨）
2. **第二波**：DecisionDraft + Portfolio（核心闭环 panel）
3. **第三波**：Quote + Chart + Article + Table + WatchList（数据 panel）
4. **第四波**：Reasoning + Plan + ToolCall（agent 状态 panel）
5. **后续**：OrderBook + FinancialReport + Diagram

每一波完成后可以做一次完整的 wireframe + 视觉稿评审。

## 21. 后续依赖文档

- `frontend/tech-stack.md` —— 具体库选型 + 项目脚手架
- `p1-spec/api.md` —— 每个 panel 订阅的事件 schema
- `p1-spec/agent-loop.md` —— agent 决定何时召唤哪个 panel 的逻辑
