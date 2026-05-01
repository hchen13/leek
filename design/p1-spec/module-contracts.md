# P1 Spec — Composable Panel & Module Contracts

> Canvas Reasoning DAG 上每个 typed 节点 = 一个 Panel；每个 Panel = chrome + 可组合 modules。
> 本文是 **后端工具结果 / agent 输出 → 前端 Panel 渲染** 的 single source of truth schema。

依赖：
- [`agent-loop.md`](agent-loop.md)（DAG 节点生成时机；§8）
- [`api.md`](api.md)（`panel_open` / `panel_update` event payload；§6）
- [`tools.md`](tools.md)（每个工具 output → 哪种 module）
- [`../frontend/panels.md`](../frontend/panels.md)（visual / interaction 设计；本文不重复）

前端实现：[`frontend/web/src/components/Panel.tsx`](../../frontend/web/src/components/Panel.tsx)——这是 schema 的 runtime truth；改 schema 必须同步改 Panel.tsx。

## 1. Composable panel 模型

```
Panel
├── kind:     PanelKind          // 决定 chrome 的 dot 颜色 + 头部 typography
├── title:    string             // chrome 上显示的标题
├── sub?:     string             // chrome 上的小副标题
├── state?:   PanelState         // chrome 上的状态边框 / 动效
├── modules:  Module[]           // body 中可组合的渲染单元（顺序渲染）
└── layout:   { x, y, w, h }     // 服务器初始 layout hint（前端 DAG 布局算法可 override）
```

一个 Panel 可含多个 Module——例：

- 一个 `fundamentals` panel = `[ {kind:"kv"}, {kind:"cmp"} ]`
- 一个 `valuation` panel = `[ {kind:"valuation"}, {kind:"cites"} ]`
- 一个 `decision` panel = `[ {kind:"decision"} ]`（典型只有一个 module）

模块组合规则：
- 同 panel 中的 module 共享 chrome + state，垂直堆叠
- module 间通过 CSS gap 视觉分隔，不画硬分割线
- Panel 的 height 是模块累加 height（前端 layout 算）

## 2. Panel 与 Reasoning DAG 节点的关系

DAG 节点（event `reasoning_dag_node`）与 Panel（event `panel_open`）是 1:1 映射：

| Reasoning DAG node 来源 | Panel kind | 典型 modules |
|--|--|--|
| tool: `quote.get`              | `quote-card`   | `[quote]` |
| tool: `chart.ohlc`             | `chart`        | `[candles]` |
| tool: `financials.history`     | `fundamentals` | `[kv]` |
| tool: `financials.compare`     | `compare`      | `[cmp]` |
| tool: `corpus.search`          | `corpus`       | `[cites]` |
| tool: `corpus.read`            | `evidence`     | `[pdf]` `[cites]` |
| tool: `news.search` / `feed`   | `news`         | `[news]` |
| tool: `filings.list`           | `news`         | `[news]` |
| tool: `filings.read`           | `evidence`     | `[pdf]` |
| tool: `vault.watchlist.get`    | `watch`        | `[rows]` |
| tool: `vault.holdings.current` | `watch`        | `[rows]` `[kv]` |
| subagent (`subagent.spawn`)    | `subagent`     | `[sub]` |
| subagent return: `valuation_*` | `valuation`    | `[valuation]` `[cites]` |
| tool: `decision.draft`         | `decision`     | `[decision]` |
| tool: `review.draft`           | `evidence` 或 `fundamentals` | `[pdf]` `[kv]` `[cites]` |

事件流：DAG 节点诞生时同步 emit `panel_open`；前端用 `panel_id == reasoning_dag_node.id` 链接两者。后续状态更新走 `panel_update`（JSON Merge Patch）。

## 3. Panel chrome schema

```typescript
interface Panel {
  panel_id: string;           // UUID v7；与 reasoning DAG 节点 ID 同源
  kind: PanelKind;
  title: string;
  sub?: string;
  state?: PanelState;
  modules: Module[];
  layout?: LayoutHint;        // 服务器初始 layout；前端 DAG 算法可重排
  pinned?: boolean;           // 用户钉住的 panel 不被 GC
  version: number;            // monotonic，panel_update 用于乱序检测
  created_at: string;
  updated_at: string;
}

type PanelState =
  | "incoming"     // 节点刚创建，agent 正在生成内容
  | "streaming"    // 内容流式到达（如 candles 一根根画）
  | "settled"      // 内容完整、稳定
  | "stale"        // 数据过时（如 quote 超过 60s 未刷新）
  | "error"        // tool call 失败
  | "pinned";      // 用户钉住

interface LayoutHint {
  x: number;       // canvas 内 px 坐标（前端 DAG 布局算法可 override）
  y: number;
  w: number;
  h: number;
}

type PanelKind =
  | "quote-card"   // 实时报价
  | "fundamentals" // 财务（三表 / KV）
  | "chart"        // K 线 / OHLC / 分时
  | "evidence"     // 一手文档（PDF / 公告 / corpus 全文）
  | "corpus"       // corpus 引用（cites 列表）
  | "compare"      // 对比表格
  | "news"         // 新闻 / 公告 / 研报
  | "subagent"     // 派遣的 subagent 容器
  | "valuation"    // 估值阶梯（DCF / 倍数）
  | "decision"     // 决策草稿（deliverable）
  | "watch";       // 自选股 / portfolio rows
```

`PanelKind` 决定 chrome 的视觉调性（dot 颜色映射见 Panel.tsx `KIND_META`）；不影响 modules 的语义（一个 fundamentals panel 也可以放 cmp module 而不是 kv，只是 chrome label 不同）。

## 4. Module schema 详解

Module 是离散的渲染单元，用 discriminated union（按 `kind` 字段区分）：

```typescript
type Module =
  | QuoteModule | KvModule | CandlesModule | PdfModule
  | CitesModule | NewsModule | RowsModule | CmpModule
  | SubModule | ValuationModule | DecisionModule;
```

后端 Rust 用 `#[serde(tag = "kind")]` 序列化保持与本 schema 一致。

### 4.1 `quote` — 实时报价

```typescript
interface QuoteModule {
  kind: "quote";
  data: {
    sym: string;        // ticker，如 "NVDA"
    price: number;
    chg: number;        // 价格变动绝对值
    chgPct: number;     // 涨跌百分比，如 2.13 表示 +2.13%
    ts: string;         // ISO8601 行情时间
    venue: string;      // 交易所，如 "NASDAQ"
  };
}
```

### 4.2 `kv` — Key-value 表

```typescript
interface KvModule {
  kind: "kv";
  title?: string;
  rows: Array<[
    string,        // 标签（如 "市盈率(TTM)"）
    string,        // 值（已格式化的字符串，如 "32.4x"）
    string?        // 可选 CSS class："up" / "dn" / "warn" 用于上色
  ]>;
}
```

### 4.3 `candles` — K 线

```typescript
interface CandlesModule {
  kind: "candles";
  w: number;            // 渲染 SVG 宽度 px（前端 layout 给）
  h: number;            // 高度
  sym?: string;
  range?: string;       // 时间窗口标签，如 "1Y" / "3M"
  price?: number;       // 当前价（标线）

  // P1 fixture：deterministic 假数据
  seed?: number;        // 固定随机种子
  base?: number;        // 起价

  // M5 真实数据接入：
  candles?: Array<{
    ts: string;
    open: number;
    high: number;
    low: number;
    close: number;
    volume: number;
  }>;
}
```

> P1 用 `seed + base` 让前端 deterministic 生成；M5 后端接入后改用 `candles` 数组。
> 两种形态在 schema 中并存，Panel.tsx 优先使用 `candles` 数组（如有），fallback 到 seed/base。

### 4.4 `pdf` — 一手文档展示

```typescript
interface PdfModule {
  kind: "pdf";
  doc: {
    title: string;
    intro: string;
    body1: string;
    highlight?: string;  // 高亮段
    body2: string;
    body3: string;
    lbl: string;         // 文档标识，如 "10-K · FY2024" / "p.3"
  };
  snippets: Array<{
    before: string;      // 引文前段
    mark: string;        // 高亮命中部分
    after: string;       // 引文后段
    cite: string;        // 引文来源标识
  }>;
}
```

### 4.5 `cites` — Corpus 引用

```typescript
interface CitesModule {
  kind: "cites";
  items: Array<{
    tier:
      | "principles-wikis"
      | "principles-sources"
      | "knowledge-wikis"
      | "knowledge-sources";    // 4-cluster，与 corpus-brain 对齐
    path: string;        // wikilink path，如 "wikis/principles/concepts/margin-of-safety"
    title: string;       // 文档标题（来自 frontmatter）
    quote?: string;      // 可选引文片段
  }>;
}
```

`tier` 决定 cite-row 左侧 pin 颜色，与 corpus-brain widget 4-cluster 调色一致。每条 cite 触发对应的 `corpus_node_activated` 事件（见 corpus-brain 后端规约）。

### 4.6 `news` — 新闻 / 公告

```typescript
interface NewsModule {
  kind: "news";
  items: Array<{
    ts: string;          // ISO8601
    src: string;         // 来源，如 "Bloomberg" / "SEC 8-K"
    head: string;        // 标题
    imp: "high" | "mid" | "low";  // 重要性
  }>;
}
```

### 4.7 `rows` — 紧凑行（Watchlist / Portfolio）

```typescript
interface RowsModule {
  kind: "rows";
  rows: Array<{
    sym: string;         // ticker
    name: string;        // 简写名
    px: number;          // 当前价
    ch: number;          // 涨跌百分比
    sparkData?: number[]; // M5 实接入；P1 fixture 用 sym 做 deterministic 种子
  }>;
}
```

### 4.8 `cmp` — 多列对比表

```typescript
interface CmpModule {
  kind: "cmp";
  headers: string[];     // 列头，如 ["指标", "NVDA", "AMD", "TSM"]
  rows: Array<{
    hl?: boolean;        // 高亮行（用于强调最重要的 1-2 行）
    cells: Array<{
      v: string;         // 单元格内容（已格式化）
      cls?: string;      // CSS class："up" / "dn" / "warn" / "best"
    }>;
  }>;
}
```

### 4.9 `sub` — Subagent 容器

```typescript
interface SubModule {
  kind: "sub";
  data: {
    role: string;        // subagent spec 的 human label，如 "估值小组 (DCF)"
    task: string;        // scope.goal 摘要
    progress: number;    // 0..1
    step: number;        // 当前 turn
    total: number;       // 预期最大 turns
    tools: number;       // 已调用 tool count
    elapsed: string;     // 人读时长，如 "12s"
  };
}
```

随 subagent_run 的 `subagent_progress` event 流式更新（panel_update 整体替换 modules 数组）。

### 4.10 `valuation` — 估值阶梯

```typescript
interface ValuationModule {
  kind: "valuation";
  steps: Array<{
    k: string;           // 项目名，如 "FCF 2024" / "WACC"
    v: string;           // 已格式化值，如 "$28.4B" / "9.5%"
    tot?: boolean;       // true = 总计行（视觉强调）
  }>;
  target?: number;       // 估值目标价（占位字段，前端待渲染）
  current?: number;      // 当前价对照
}
```

典型搭配：valuation panel 同时含 `valuation` + `cites`（估值过程 + 引用的 corpus 概念）。

### 4.11 `decision` — 决策草稿

```typescript
interface DecisionModule {
  kind: "decision";
  data: {
    verdict: "BUY" | "SELL" | "HOLD" | "TRIM" | "ADD";
    sym: string;
    confidence: number;  // 0..1
    gist: string;        // 一句话理由
    params: Array<{
      k: string;         // 参数名，如 "Position size" / "Stop loss" / "Horizon"
      v: string;         // 值，如 "+15 shares" / "$440" / "120d"
    }>;
  };
}
```

decision panel 是仪式性 deliverable——chrome 上有 "REVIEW & SUBMIT" / "EDIT PARAMS" / "PIN" 按钮（由 Panel.tsx 渲染）。Confirm 流程见 [`api.md`](api.md) §4.6。

## 5. Tool result → Module 映射（落地表）

| Tool | 主 Module | 选配 Module |
|--|--|--|
| `quote.get`               | `quote`     | — |
| `chart.ohlc`              | `candles`   | — |
| `financials.history`      | `kv`        | — |
| `financials.compare`      | `cmp`       | — |
| `corpus.search`           | `cites`     | — |
| `corpus.read`             | `pdf`       | `cites`（related links） |
| `news.search` / `news.feed` | `news`    | — |
| `filings.list`            | `news`      | — |
| `filings.read`            | `pdf`       | — |
| `vault.watchlist.get`     | `rows`      | — |
| `vault.holdings.current`  | `rows`      | `kv`（aggregate stats） |
| `subagent.spawn`          | `sub`       | — |
| `valuation_dcf`(subagent return) | `valuation` | `cites` |
| `decision.draft`          | `decision`  | — |
| `review.draft`            | `pdf` 或 `kv` | `cites` |

主 module = 该 tool 必出的 1 个；选配 module = agent 自由判断是否搭配。

## 6. Panel 事件协议

panel lifecycle 通过 4 个 event 驱动（详见 [`api.md`](api.md) §6.3）：

| Event | 时机 | Payload |
|--|--|--|
| `panel_open` | tool_call 启动 / agent 显式 panel 召唤 | `{ panel_id, kind, title, sub?, modules, layout_hint?, version: 1 }` |
| `panel_update` | tool result / subagent 进度 / agent 修订 | `{ panel_id, patch: JSONMergePatch, version }` |
| `panel_close` | agent 显式关闭（少见） | `{ panel_id, reason? }` |
| `panel_pinned` | 用户钉住 | `{ panel_id, pinned: true }` |

### 6.1 `panel_update` 的 JSON Merge Patch 语义

按 [RFC 7396](https://datatracker.ietf.org/doc/html/rfc7396)：

- 顶层字段（`title` / `sub` / `state`）直接 replace
- `modules` 数组按**整体替换**——不做 array element merge（modules 顺序 / 长度变化太频繁，diff merge 复杂度高）
- 单个 module 内部字段可以 deep merge（前端 reconciler 处理）

例：subagent_progress 更新 progress 与 step：

```json
{
  "panel_id": "p_abc",
  "patch": {
    "modules": [
      { "kind": "sub", "data": { "progress": 0.6, "step": 3, "tools": 7, "elapsed": "18s" } }
    ]
  },
  "version": 5
}
```

完整的 modules 数组必须重发——这避免 array index drift 导致的 merge 错乱。

### 6.2 `version` 的并发控制

每个 panel 有 monotonic increasing `version`：

- 初始 panel_open → version=1
- 每次 panel_update → server 端 ++version（原子）
- 前端忽略 version <= current 的 patch（防乱序）
- gateway 重连续传时按 version sort

### 6.3 与 vault 的持久化

每次 `panel_open` 写入 `vault.artifacts`（[`data-schema.md`](data-schema.md) §2.2）：

- `kind: "panel:<panel_kind>"`，如 `"panel:quote-card"` / `"panel:decision"`
- `payload_json`: 完整 Panel 序列化
- `parent_artifact_id`: 关联的 reasoning DAG 节点 ID

每次 `panel_update` 更新该 artifact 行（不写入新行）。

## 7. 前端实现对照

`frontend/web/src/components/Panel.tsx` 是 schema 的 runtime truth：

| Schema 字段 | Panel.tsx 对应 |
|--|--|
| `Panel.kind`            | `PanelProps.kind` (PanelKind union) |
| `Panel.modules[]`       | `PanelProps.modules` (Module union) |
| `Panel.layout.{x,y,w,h}` | `PanelProps.{x,y,w,h}` |
| `Panel.state`           | `PanelProps.state`（drives `data-state` attr） |
| 各 Module 字段          | 各 `Mod*` 函数的 props |

**Schema 改动协议**：

1. 修改本文档的 schema
2. 同步修改 `Panel.tsx` 的 type alias + 对应 renderer
3. 同步修改 `tools.md` 中 tool result schema（如果 tool 输出形态变化）
4. 后端 panel emitter 单元测试覆盖新 schema

## 8. 实施 checklist

### 后端
- [ ] `crate::panel::Panel` struct + `PanelKind` enum + `Module` discriminated union
- [ ] 序列化用 `#[serde(tag = "kind")]`，与本文 schema 严格 match
- [ ] `tool_result_to_module`：每种 tool 对应一种 module
- [ ] `panel_open` / `panel_update` event emitter（version monotonic）
- [ ] panel 持久化层（写 `vault.artifacts`，kind=`panel:<panel_kind>`）

### 前端
- [x] Panel.tsx 11 module renderer 实现（已 done，与本文 schema 对齐）
- [ ] panel store：`createSignal<Map<panel_id, Panel>>`
- [ ] event reducer：监听 `panel_open` / `panel_update` / `panel_close`
- [ ] JSON Merge Patch reducer（modules 整体替换，其他字段 deep merge）
- [ ] version 序号检查（discard stale update）
- [ ] DAG layout 算法 override `layout_hint`

### 测试
- [ ] 后端：每种 tool result → module 转换的 round-trip JSON 校验
- [ ] 前端：每种 module renderer 的 snapshot test
- [ ] e2e：完整 task → panel_open + 多次 panel_update → 最终 modules 一致
