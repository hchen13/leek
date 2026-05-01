# L.E.E.K Frontend Panels

> 主 canvas 是一棵活的 Reasoning DAG，每个 panel 是 DAG 上的一个 typed 节点。本文档定义所有节点类型 + 整棵 DAG 的视觉、布局、交互规则。

先读 [`concept.md`](concept.md) 再读这份。

> **数据契约**（panel chrome + 11 typed module 的 JSON schema、tool result → module 映射、panel_open / panel_update payload）见 [`../p1-spec/module-contracts.md`](../p1-spec/module-contracts.md)。本文专注 visual / interaction craft，不重复 schema。

## 1. 核心心智模型：Canvas = Reasoning DAG，Panel = Typed Node

L.E.E.K 的主 canvas **不是 dashboard 风格的多窗格摆放**，也不是 chat 流的纵向消息列表。它是一棵**活的、可视化的、流动的 Reasoning DAG**——agent 的全部工作过程（思考 / 调工具 / 看数据 / 引用 corpus / 派遣 subagent / 提交 deliverable）都是 DAG 上长出来的 typed 节点，节点之间的边是因果连接。

```
   ◆ user_input ────────────────────────────────────┐
                                                    │
   ●─→ ToolCall: vault.holdings.current             │
        │                                           │
        ▼                                           │
   ●─→ Observation: 已有 50 股                       │
        │                                           │
   ●─→ ToolCall: quote NVDA                         │
        │                                           │
        ▼                                           │
   ┌─────────────────────────────┐                  │
   │ Quote 节点（typed: quote）   │ ← 节点本身就是    │
   │  NVDA $480.20 ↑ 2.1%        │   "Quote panel"  │
   │  ━━━━━━━ sparkline           │                  │
   └─────────────────────────────┘                  │
        │                                           │
   ┌─────────────────────────────┐                  │
   │ 📌 Subagent: valuation_dcf   │                  │
   │   turn 1/2/3 ✓               │                  │
   │   result: DCF $520           │                  │
   └─────────────────────────────┘                  │
        │                                           │
   🧠 Corpus: margin-of-safety ◀── 同时激活 Brain    │
        │                                           │
   ●─→ Reasoning: 估值偏高，但护城河...               │
        │                                           │
   ┌─────────────────────────────┐                  │
   │ DecisionDraft 节点          │ ← 这也是节点      │
   │  long +15 股 stop $440       │                  │
   │  [Confirm] [Discuss] [Reject]│                  │
   └─────────────────────────────┘──────────────────┘
```

### 1.1 关键含义

- **没有"召唤一个 Quote panel"——只有"DAG 长出一个 Quote 节点"**。Quote 是节点的 typed 视觉，不是独立摆放的卡片
- **没有用户拖动 / 钉住 / 关闭 panel 的概念**——节点位置由 DAG 布局算法决定，用户只能 zoom / 折叠分支 / 全屏某节点
- **没有"打开 / 关闭"生命周期**——节点出现后是 task 历史的一部分，不可关闭（可折叠）
- **canvas 是 task scoped 的**——每个 task 一棵 DAG；切换 task = 切换 DAG

### 1.2 节点的 typed 视觉

每个节点根据 `kind` 有不同的视觉形态。常见类型：

| 节点 kind | 视觉形态 | 大小（默认）|
|--|--|--|
| `user_input` | 用户原始 message bubble | 小 |
| `task_start` | 任务起点节点（◆ 形状 + title）| 小 |
| `thinking` | 思考片段气泡（流式逐字显示） | 中 |
| `tool_call` | 工具调用容器（含参数 + 结果摘要） | 小 |
| `observation` | 工具调用结果的 typed 视觉（见下） | 中-大 |
| `corpus_ref` | 🧠 corpus 节点 chip（同时激活 CorpusBrain） | 极小 |
| `subagent_branch` | 📌 subagent 容器（封装内部 turn 序列） | 中 |
| `decision_draft` | 决策草稿 form（◆ 形状） | 大 |
| `review_draft` | 复盘草稿（◆ 形状） | 大 |
| `final_reply` | 最终回复（◆ 形状） | 中 |

`observation` 节点根据数据类型再分 typed 视觉（**这就是过去的 Quote / Chart / Article / Table panel**——它们现在是 observation 节点的 typed 渲染）：

| Observation 子 typed | 即过去的 panel |
|--|--|
| `quote` | Quote |
| `chart_ohlc` | Chart |
| `article` | Article |
| `table` | Table |
| `financial_report` | FinancialReport |
| `orderbook` | OrderBook |
| `diagram` | Diagram |
| `portfolio_snapshot` | Portfolio（节点形态，非常驻） |

### 1.3 节点的生命周期

```
            agent 流程触发
                  │
                  ▼
          ┌──────────────┐
          │  appearing   │  fade + scale 入场（150ms）
          └───────┬──────┘
                  │
                  ▼
          ┌──────────────┐
          │  active      │  当前活跃节点持续脉冲；
          │              │  数据流式填充（如 thinking 逐字、tool args delta）
          └───────┬──────┘
                  │ 数据完整
                  ▼
          ┌──────────────┐
          │  ready       │  完成态，可交互（hover / 点击）
          └───────┬──────┘
                  │
                  ├─→ update events → 节点局部刷新（如 quote tick）
                  └─→ collapsed → 用户折叠该节点 / 子分支
```

节点不会"关闭"——它们是 task 历史的永久部分，存进 `vault.reasoning_dag_traces` + `vault.artifacts`。

### 1.4 整棵 DAG 的布局

- **算法**：分层布局（top-down 或 left-right），同层水平展开，子分支（如 subagent / 多线推理）缩进展示
- **不允许用户拖动节点位置**——位置由布局算法决定
- **允许的用户控制**：
  - 滚动 / 平移整棵树
  - zoom in/out（鼠标滚轮）
  - 折叠 / 展开某分支（如 subagent_branch 默认折叠为容器，点击展开内部 turn）
  - 节点 hover → 显示完整摘要 popover
  - 节点点击 → "fullscreen 该节点"模式（暂时把这个节点放大到主区域，按 Esc 退出）
- **当前活跃节点**自动滚动到视口中心（可关闭跟随）

### 1.5 节点的事件订阅

事件协议见 `p1-spec/api.md`，节点 kind 对应订阅：

```
NodeKind             → 订阅事件
─────────────────────────────────────────────
thinking             → agent_thinking_delta
tool_call            → tool_call_args_delta, tool_call_result
observation:quote    → tool_call_result (typed=quote) + tick.<ticker>
observation:chart    → tool_call_result + ohlc.<ticker>.<period>
corpus_ref           → corpus_node_activated（推 CorpusBrain）
subagent_branch      → subagent_started, subagent_progress, subagent_completed
decision_draft       → deliverable_draft_*
```

### 1.6 不属于 Reasoning DAG 的元素

不是所有 UI 元素都是 DAG 节点。以下是独立 panel / 区域：

| 元素 | 位置 | 性质 |
|--|--|--|
| **Chat 主轴** | 左侧 | 用户输入 + agent final reply 的简洁线性流 |
| **Reasoning DAG** | 主区域（右 / 中） | 本文档的核心，agent 工作的可视化 |
| **CorpusBrain** | 侧栏 ambient | corpus 全景知识图谱 + 激活动效（详见 §3） |
| **Watchlist 摘要** | 右栏 widget | 自选股快照（持续可见，非节点） |
| **Portfolio 摘要** | 右栏 widget | 当前持仓快照（持续可见，非节点） |
| **TaskBar** | 顶部 / chat 主轴顶端 | 当前 task status chip + 干预按钮（详见 §18b） |
| **Settings** | 单独 page | ProviderConfig / CharterEditor 不在主 canvas |

## 2. 元素清单（P1）

分两类：**DAG 节点 typed**（生在 canvas 里）和**独立元素**（不在 DAG 中）。

### 2.1 DAG 节点 typed 清单

| 节点 kind | 用途 | 渲染 | 详细章节 |
|--|--|--|--|
| `user_input` | 用户原始 message | DOM | §11 |
| `task_start` | task 起点节点 | DOM | §11 |
| `thinking` | 思考片段（流式逐字） | DOM | §12 |
| `tool_call` | 工具调用容器 | DOM | §14 |
| `observation:quote` | 行情快照 + tick stream | DOM (高频 ref) | §5 |
| `observation:chart_ohlc` | K 线 / 分时（含指标） | Canvas (lightweight-charts) | §6 |
| `observation:orderbook` | 盘口 + 成交流 | Canvas | §7 |
| `observation:financial_report` | 财务三表 | DOM (table) | §8 |
| `observation:article` | 新闻 / 公告 / 研报 | DOM (markdown) | §9 |
| `observation:table` | 通用表格 | DOM (TanStack Table) | §10 |
| `observation:diagram` | SVG / Mermaid | SVG | §11b |
| `observation:portfolio_snapshot` | 当前持仓节点视图 | DOM (table) | §17 |
| `corpus_ref` | corpus 引用 chip（激活 Brain） | DOM | §1 内嵌 |
| `subagent_branch` | subagent 容器 + 内部 turn | DOM + 折叠 | §1 内嵌 |
| `decision_draft` | 决策草稿 form（deliverable） | DOM (form) | §15 |
| `review_draft` | 复盘草稿（deliverable） | DOM (form) | §18e |
| `research_brief` | 调研简报（deliverable，节点内嵌多子节点） | DOM | §11b |
| `comparison` | 多标的对比（deliverable） | Table + Chart | §11b |
| `final_reply` | 最终回复 | DOM | §11 |
| `plan` | 执行计划（typed plan，节点形态） | DOM | §13 |

### 2.2 独立元素（不在 DAG 中）

| 元素 | 类型 | 详细章节 |
|--|--|--|
| **CorpusBrain** | 侧栏 ambient panel | §3 |
| **Chat 主轴** | 左侧线性消息流 | §18a |
| **TaskBar** | chat 主轴顶部 status chip + 干预区 | §18b |
| **Watchlist 摘要** | 右栏 widget | §16 |
| **Portfolio 摘要** | 右栏 widget | §17 |
| **ProviderConfig** | Settings page | §18f |
| **CharterEditor** | Settings page | §18g |

### 2.3 P1 范围

**P1 必做的 DAG 节点 typed**（自然出现在 agent 工作流中）：
user_input、task_start、thinking、tool_call、observation:quote / chart_ohlc / article / table / financial_report / portfolio_snapshot、corpus_ref、subagent_branch、decision_draft、review_draft、final_reply、plan

**P1 必做的独立元素**：CorpusBrain、Chat 主轴、TaskBar、Watchlist / Portfolio 摘要、ProviderConfig、CharterEditor

**P1 nice-to-have**：observation:orderbook / diagram、research_brief、comparison

**P2 候选**：observation:heatmap / correlation_matrix

下面逐类展开。

---

## 3. CorpusBrain ⭐ — 核心叙事 panel

**这是 L.E.E.K 的产品 signature。投入最多 craft 在这里。**

> 后端规约（graph 数据来源、节点/边 schema、激活事件协议、persistence）见 [`../p1-spec/corpus-brain.md`](../p1-spec/corpus-brain.md)。本节专注 visual / interaction craft。

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

## 4. Reasoning DAG 整体（canvas 本体）

> 注：Reasoning DAG **不是一个 panel**，它是 canvas 本体。本节定义整棵 DAG 的视觉、数据形态、渲染、动效与持久化。各 typed 节点的具体形态在 §5+ 各章节详述。

### 4.1 数据形态

```typescript
type ReasoningNode = {
  id: string;
  kind: "user_input" | "task_start" | "thinking" | "tool_call" | "observation"
      | "corpus_ref" | "subagent_branch" | "subagent_result"
      | "decision_draft" | "review_draft" | "research_brief" | "comparison"
      | "final_reply" | "plan";
  observation_typed?: "quote" | "chart_ohlc" | "orderbook" | "financial_report"
      | "article" | "table" | "diagram" | "portfolio_snapshot";  // kind=observation 时
  title: string;
  details?: string;
  payload_json?: object;          // observation 节点的具体数据
  ts: string;
  status: "appearing" | "active" | "ready" | "errored" | "collapsed";
  subagent_run_id?: string;       // subagent_branch / subagent_result 时
};

type ReasoningEdge = {
  from: string;
  to: string;
};
```

### 4.2 渲染策略

**DOM + SVG 混合**：
- 节点：DOM box（每种 typed 节点有独立组件，参见 §5+）
- 边：SVG path（带 traveling pulse 动效）
- 整个 canvas 用一个共享坐标系，节点位置由布局算法决定
- 节点过密时局部用 Canvas 加速渲染（可选优化，P1 默认 DOM）

理由：节点是 typed 视觉（每种 panel 不同布局，DOM 最灵活）；典型 task DAG 有 10-50 节点，DOM 性能完全够用；动效与 hover/点击交互在 DOM 上易实现。

### 4.3 流式展开动效

- **节点入场**：fade + scale 150ms
- **边入场**：stroke draw 200ms
- **当前活跃节点**：持续脉冲 outline（直到下一节点接力）
- **边的 traveling pulse**：沿边路径走光点动效（200ms / 边）
- **数据流式填充**：thinking 节点逐字、tool args delta 累积显示

### 4.4 整棵 DAG 的交互

- **节点 hover** → 高亮所有上下游节点（path 高亮 + 其他节点淡化）
- **节点点击** → fullscreen 该节点（暂时把这个节点放大到主区域，按 Esc 退出）
- **节点 hover popover** → 显示节点完整摘要（thinking 全文 / tool 参数 / observation 详细）
- **滚动 / 平移** → 浏览整棵 DAG（自动跟随当前活跃节点可关闭）
- **滚轮缩放** → zoom in/out
- **折叠 / 展开 subagent_branch** → 点击 📌 容器节点切换内部 turn 序列展示
- **corpus_ref 点击** → CorpusBrain 同步高亮
- **"重放" 按钮**（task 完成后）→ 回到 task 开始，再放一遍动效

### 4.5 Subagent 子分支的可视化（关键视觉）

当 main agent 调 `subagent.spawn` 时：

1. canvas 加一个 `subagent_branch` 节点（容器形态，📌 图标 + spec_name + 实时 progress）
2. Subagent 内部的 turn / tool call / observation **默认折叠在容器内**——它们是 subagent 的内部细节
3. 用户点击容器节点 → 展开，显示内部 turn 序列：
   - "Turn 1: financials.history NVDA → got 5y data"
   - "Turn 2: indicator.compute MA200 → ok"
   - "Turn 3: synthesizing DCF model → result $520"
4. Subagent 完成时容器状态变 ready，加一个 `subagent_result` 节点（含 summary）
5. main agent 继续工作，新节点连边到 subagent_result

**这是 manager + team framing 的关键视觉**：用户看到团队"派遣了估值小组"而不是黑盒。同时 subagent 内部细节是可访问但默认折叠的——不污染主 DAG 的整洁。

### 4.6 持久化

- 整个 DAG（节点 + 边 + 时序）存 `vault.reasoning_dag_traces`（每 task 一条）
- 各节点的 `payload_json` 存 `vault.artifacts`（kind=panel:<typed>）
- 用户后来打开同一 task 重看时，能完整重建（含动效回放选项）

### 4.7 与 CorpusBrain 的联动

- 点击 DAG 中的 `corpus_ref` 节点 → CorpusBrain 中对应节点高亮 + 镜头平移过去
- 点击 CorpusBrain 节点 → 如果当前 DAG 引用过该节点，那个 corpus_ref 节点高亮

---

## 5. Quote 节点（observation:quote）

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

---

## 18a. Chat 主轴 — 用户输入与 agent 最终回复的简洁线性流

### 18a.1 用途

L.E.E.K 唯一的"对话"区域。设计上**极简**——只显示用户输入和 agent 的最终回复，**不显示中间过程**（中间过程全在 canvas DAG 上展开）。

### 18a.2 视觉

```
┌────────────────────────────────────────────────────────┐
│ ── TaskBar (顶部) ──────────────────────────────────── │
│ ⟳ in_progress · 评估 NVDA 加仓 · 24s                   │
│ [追加约束] [中断]                                       │
├────────────────────────────────────────────────────────┤
│                                                        │
│  你: NVDA 现在能加仓吗？我已经有 50 股                  │
│                                                        │
│  L.E.E.K: 基于你的当前持仓和 margin-of-safety 框架,   │
│           我建议加仓 15 股，止损 $440，期限 120 天。    │
│           完整决策草稿已生成在画布上 →                  │
│                                                        │
│  你: 也帮我看一下 BABA 的对比                          │
│                                                        │
│  L.E.E.K: 收到，把 BABA 加入分析。                     │
│                                                        │
├────────────────────────────────────────────────────────┤
│ [输入框]                                                │
│  ▶ 给团队下达任务、追问、或提建议...        [Send ↵]   │
└────────────────────────────────────────────────────────┘
```

### 18a.3 关键设计

- **不像 ChatGPT 那样塞中间过程**：用户不会在 chat 里看到"让我先查一下..."、"调用 corpus.search..."、"分析中..."这种内容——那些都在 canvas DAG 上
- **agent 的回复是高度浓缩的**：典型 1-3 句，含一个"完整结果在画布上 →"指引
- **mention chip 仍然支持**：`@NVDA` / `@portfolio:current` / `@corpus:margin-of-safety` / `@decision:abc123`，输入时触发 chip
- **首条 user message 隐式触发 task 创建**：不需要 user 填 form。Agent 自己从 user 输入提取 task.goal / constraints
- **后续追问**：在已有 task thread 内继续输入 = 自然语言追问；agent 决定是"追加约束 inject 到当前 task"还是"开新 task"

### 18a.4 渲染

DOM。Agent 输出流式逐字 / 逐 token 显示。

### 18a.5 交互

- 输入框支持 mention chip
- Cmd+Enter 发送
- 长输入支持多行（按 Shift+Enter 换行）
- 历史消息可滚动
- 点击 agent 回复中的"完整结果在画布上 →"指引 → 主区域定位到 deliverable 节点

---

## 18b. TaskBar — 当前任务状态 + 干预

### 18b.1 用途

显示当前活跃 task 的 status + 提供轻量级干预入口（追加约束 / 中断）。位于 chat 主轴顶端，**不是首页主区域**——不让 task 概念变重。

### 18b.2 视觉

```
不同 status 下：

[in_progress]
┌────────────────────────────────────────────────────────────┐
│ ⟳ in_progress · 评估 NVDA 加仓 · turn 3/5 · 24s           │
│ Agent 当前: corpus.search "margin of safety"               │
│ [追加约束]  [中断]                                          │
└────────────────────────────────────────────────────────────┘

[awaiting_user]
┌────────────────────────────────────────────────────────────┐
│ ❓ awaiting_user · 评估 NVDA 加仓                          │
│ "你说的 NVDA 是 Nvidia 还是 Navidec？" ← 答即可继续         │
└────────────────────────────────────────────────────────────┘

[delivered]
┌────────────────────────────────────────────────────────────┐
│ ✓ delivered · 评估 NVDA 加仓                                │
│ DecisionDraft 已生成在画布上 → [Confirm] [Discuss] [Reject] │
└────────────────────────────────────────────────────────────┘

[idle - 没有 active task]
┌────────────────────────────────────────────────────────────┐
│ idle · 提个问题或下达任务，团队待命                         │
└────────────────────────────────────────────────────────────┘
```

### 18b.3 数据形态

订阅当前 session 的最新 task status：`task_status_changed` / `agent_message_delta` / `clarification_requested`。

### 18b.4 渲染

DOM (slim bar)。高度紧凑（~40-60 px）。

### 18b.5 交互

- **[追加约束]** → 弹一个小输入框，用户输入"也考虑 BABA"——这是 control 命令，不是 chat message。Agent 在下一轮 LLM call 看到
- **[中断]** → 立即发 interrupt control，task 进 cancelled。canvas DAG 保留，可继续 fork
- **status chip 点击** → 跳到 canvas 上当前活跃节点
- **awaiting_user 的问题** → 用户直接在 chat 输入框回答即可

### 18b.6 多 task 并行（P1.5）

P1 默认单 task 模式。P1.5 支持多 task 并行时 TaskBar 变 dropdown 切换。

---

## 18e. Review — 复盘 deliverable（DAG 节点）

### 18e.1 用途

agent 生成的复盘草稿，用户编辑 + confirm。

### 18e.2 视觉

```
┌─────────────────────────────────────────────────────────────┐
│ Review · NVDA Decision (2026-02-01)              [─] [×]   │
├─────────────────────────────────────────────────────────────┤
│  关联决策: NVDA long 50 股 @ $420 (3 个月前)                │
│                                                             │
│  当时 vs 现在                                                │
│  ┌──────────────────────────────────────────────────────┐  │
│  │ 价格:        $420 → $480  (+14.3%)                   │  │
│  │ Stop:        $380 (未触发)                            │  │
│  │ 市场环境:    AI 半导体大涨 / 财报超预期               │  │
│  │ 当时引用:    margin-of-safety / owners-earnings       │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
│  Self Score: ◯ 1 ◯ 2 ◯ 3 ◉ 4 ◯ 5    Agent Score: 4       │
│                                                             │
│  Lessons learned (markdown)                                 │
│  ┌──────────────────────────────────────────────────────┐  │
│  │ - 当时低估了 AI 推理需求增速...                         │  │
│  │ - margin-of-safety 在确定性高的护城河公司可放宽...     │  │
│  │ - 未来类似机会应当更果断 size up...                    │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
│  Corpus inbox candidates (P1 不自动写)                      │
│   ☑ "AI 推理需求曲线的非线性"                               │
│   ☐ "技术栈锁定型护城河的估值方法"                          │
│                                                             │
│  [Discard]  [Save as Draft]  [Confirm ✓]                    │
└─────────────────────────────────────────────────────────────┘
```

### 18e.3 数据形态

参考 [`../p1-spec/data-schema.md`](../p1-spec/data-schema.md) 的 `reviews` 表 + `deliverables` (kind=review) 形态。

### 18e.4 交互

- 行内编辑 lessons / scores
- 复盘对照视图（决策时点 vs 现在）
- Corpus inbox candidates 是 checklist：用户勾选哪些"值得进 corpus"——P1 仅记录意图，不自动写文件

---

## 18f. ProviderConfig — LLM Provider 配置

### 18f.1 用途

设置 LLM provider 凭证（API key / OAuth）+ 优先级 / 降级链。是 onboarding 必经，也是 Settings page 的一部分。

详细形态见 [`../p1-spec/llm-provider.md`](../p1-spec/llm-provider.md) §10。

### 18f.2 关键 UX 约束（重申）

1. API key 输入框默认 mask
2. Test connection 是必经动作
3. Fallback chain 可视化拖拽
4. Status 三色一致（active / disabled / invalid）
5. 每个 provider 有 Disable 而非 Delete
6. OAuth re-authorize 单独按钮
7. Codex OAuth 风险提示要醒目但不恐吓

---

## 18g. CharterEditor — Team Charter 可视化编辑

### 18g.1 用途

用户编辑 Team Charter（升级版的 mandate）。是用户表达自己的入口，也是 agent 输出 deliverable 时的硬约束来源。

### 18g.2 视觉

```
┌──────────────────────────────────────────────────────────────┐
│ Team Charter                  Active · v3 · last updated 2d  │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│ Style                                                        │
│   ✓ long-term-fundamental                                    │
│   ✓ margin-of-safety                                         │
│   ✓ quality-over-cycles                                      │
│   ☐ contrarian                                               │
│   [+ Add custom style tag]                                   │
│                                                              │
│ Hard Limits                                                  │
│   Max position size:        [ 10 ] %                         │
│   Max drawdown tolerance:   [ 25 ] %                         │
│   Forbidden instruments:                                     │
│     ✓ options    ✓ leveraged_etfs    ☐ crypto                │
│                                                              │
│ Soft Preferences                                             │
│   Preferred sectors:    [tech] [consumer-staples] [+]        │
│   Avoid sectors:        [tobacco] [+]                        │
│   Avoid geos:           [russia] [belarus] [+]              │
│                                                              │
│ Work Style                                                   │
│   Decision verbosity:    ◯ brief  ◯ standard  ◉ detailed     │
│   Cite corpus always:    ◉ yes  ◯ no                          │
│   Challenge my bias:     ◉ yes  ◯ no                          │
│                                                              │
│ ⚠ 这些约束对 agent 是强制的：违反 hard_limits 会被自动调整 / │
│   触发 soft_preferences 警告会显式 flag 在 deliverable 里    │
│                                                              │
│ [Discard changes]  [Preview]  [Apply]                        │
└──────────────────────────────────────────────────────────────┘
```

### 18g.3 数据形态

参考 [`../interaction-model.md`](../interaction-model.md) §6.1 Team Charter YAML 形态。

### 18g.4 交互

- 表单式编辑 + chip 添加 / 删除
- Preview：显示如果 apply 后，最近一个 task 的 mandate_check 会怎么变（让用户感知影响）
- Apply：创建新 active charter（旧版本保留为非 active）
- 历史版本可查（Charter History sub-page）

---

## 19. 元素间交互模式

### 19.1 节点出现机制

DAG 节点由 agent loop 在工作过程中**自动**生成（详见 [`../p1-spec/agent-loop.md`](../p1-spec/agent-loop.md) §8）——不是用户主动召唤，也不是 agent 显式调用 "panel.open"。每个 LLM 流式事件 / tool call / corpus 引用 / subagent spawn / deliverable 提交都对应一个 DAG 节点的诞生。

### 19.2 联动机制

- **CorpusBrain ⟷ canvas DAG**：corpus_ref 节点点击 ↔ Brain 中对应节点高亮
- **subagent_branch 节点折叠 / 展开**：默认折叠为容器，点击展开内部 turn 序列
- **observation:quote / chart_ohlc 节点**：点击 fullscreen 看完整图表
- **decision_draft 节点**：点击 fullscreen 进入编辑 + Confirm/Reject
- **Watchlist / Portfolio 摘要 ⟷ chat 输入**：拖入 ticker → 自动插入 mention chip
- **agent 回复中的"完整结果在画布上 →"指引** → 主区域定位到对应 deliverable 节点

### 19.3 mention chip

Chat 输入框支持：

- `@NVDA` → ticker chip（绿色，可拖动）
- `@corpus:margin-of-safety` → corpus chip（紫色）
- `@decision:abc123` → decision chip（橙色，引用历史决策）
- `@task:xyz` → task chip（蓝色，引用历史任务）
- `@portfolio:current` → portfolio chip（青色，引用当前持仓）
- `@review:abc` → review chip（黄色，引用历史复盘）

mention chip 在提交时被解析成 agent 的 strong context（不是普通文本，而是 typed reference）。

## 20. P1 实施排期

**P1 必做的 DAG 节点 typed**（13 类）：
user_input / task_start / thinking / tool_call / observation:quote / chart_ohlc / article / table / financial_report / portfolio_snapshot / corpus_ref / subagent_branch / decision_draft / review_draft / final_reply / plan

**P1 必做的独立元素**（7 类）：
CorpusBrain / Chat 主轴 / TaskBar / Watchlist 摘要 / Portfolio 摘要 / ProviderConfig / CharterEditor

**P1 nice-to-have**：observation:orderbook / diagram / research_brief / comparison

**P2 候选**：observation:heatmap / correlation_matrix

UX 设计排期建议（按依赖与重要性）：

1. **第一波（产品 signature）**：CorpusBrain + Reasoning DAG 整体（含基础节点 typed：thinking / tool_call / observation:quote / corpus_ref / subagent_branch）
2. **第二波（chat 主轴 + 干预）**：Chat 主轴 + TaskBar + 输入区 mention chip
3. **第三波（核心闭环 deliverable 节点）**：decision_draft / review_draft 节点 + Portfolio 摘要
4. **第四波（数据节点完善）**：observation:chart_ohlc / article / table / financial_report
5. **第五波（agent 状态节点）**：plan / final_reply / 流式动效完善
6. **第六波（设置页）**：ProviderConfig + CharterEditor
7. **后续**：observation:orderbook / diagram、research_brief / comparison

每一波完成后可以做一次完整的 wireframe + 视觉稿评审。

## 21. 后续依赖文档

- `frontend/tech-stack.md` —— 具体库选型 + 项目脚手架
- `p1-spec/api.md` —— 节点对应的事件 schema
- `p1-spec/agent-loop.md` —— agent 工作时如何驱动节点生成
