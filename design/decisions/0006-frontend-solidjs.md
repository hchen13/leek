# ADR 0006 — 前端框架用 SolidJS

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0007](0007-event-protocol-and-transports.md)（事件协议决定前端要消费的数据流形态）

## Context

L.E.E.K 的 Web 前端是 chat-canvas 形态（chat 主轴 + 动态生成的 panels），P1 阶段将承载：

- 高频跳动的金融数字（实时 quote tick stream，单标的可能 5-20 Hz）
- 重型可视化（K 线图、orderbook、思维链 DAG with 动效、相关性矩阵等）
- 思维链 DAG 实时展开 + 节点动效（节点出现、边连接、当前活跃高亮）
- 多个并存 panel 各自独立刷新数据
- 长 session 的消息历史（数百条 message + 各种 inline artifact）

**性能是核心约束**——React 的 vdom 在每秒数十次跨多组件更新场景下不是最佳选择，需要绕过框架（`useRef` + 直接 DOM 操作 / `requestAnimationFrame`）。这能跑，但是补丁不是根治。

候选方案：

| 方案 | 性能（高频更新） | 心智模型 | 生态 / UI 库 | AI 辅助 |
|--|--|--|--|--|
| **React + Vite** | 中（需补丁） | 主流 | 极丰富（shadcn/ui） | 极强 |
| **SolidJS** | 高（fine-grained reactivity） | JSX 风格、对 React 友好 | 中等（Kobalte / Ark UI） | 中 |
| **Svelte 5 (runes)** | 高（编译时优化） | 与 React 差异较大 | 中等 | 中 |
| **WebAssembly (Leptos / Yew)** | 极高 | Rust，与 gateway 同语言 | 极弱（要自己写 UI 库） | 弱 |
| **Vanilla + Canvas-first** | 极高 | 完全可控 | 无（自己写） | 弱 |

## Decision

**前端用 SolidJS**。

技术栈基线：
- 框架：**SolidJS**
- 路由：`@solidjs/router`
- 状态管理：Solid 内建 `createSignal` / `createStore`（不引入 Zustand 类外部库）
- 样式：**Tailwind CSS**
- 组件库（headless）：**Kobalte** 或 **Ark UI**（Solid 适配）+ Tailwind 自己拼视觉
- 图标：`lucide-solid`
- 构建：**Vite**
- 类型：TypeScript
- 跨框架库（可直接用）：TanStack Table / TanStack Query / TanStack Router（如不用 Solid Router）

## 重型可视化的渲染策略

**无论用什么 UI 框架，重型可视化都走 Canvas / WebGL**——TradingView、Bloomberg Terminal 都不是用 DOM 画 K 线的。框架选择只决定 chat 主轴 + panel 容器层的细粒度更新。

P1 重型可视化候选库（具体在 `frontend/panels.md` 拍板）：
- K 线 / 分时图：**lightweight-charts**（TradingView 出品，Canvas-based，性能极好）/ **ECharts**
- 思维链 DAG with animation：**D3 + Canvas**（最灵活）/ **Cytoscape.js**（自带布局算法）/ 自己写 Canvas
- OrderBook：自己写 Canvas（实时 tick 流场景，Canvas 是唯一选择）
- 通用图表 fallback：**Apache ECharts**（成熟、文档好、覆盖图表类型最全）

Solid 与 Canvas 的胶合：用 `createEffect` 监听 signal 变化触发 Canvas redraw，或在 panel 组件里持有 ref + 直接调用 chart 库 API。

## Consequences

### 性能问题被框架原生解决

- 100 Hz 跳动的 quote 数字：`createSignal<number>(0)` + 直接绑定到 JSX 文本节点，单 signal 变化只更新对应 DOM 文本节点，无 vdom diff
- 多 panel 并存独立刷新：每个 panel 内部自己的 signal scope，互不影响
- 思维链 DAG 节点流式增加：`createStore` 的细粒度 update 只更新新节点的 DOM

### shadcn/ui 用不了

shadcn/ui 是 React 生态，不能直接用。替代方案：

- **Kobalte**（Solid 适配的 headless 组件库，仿 Radix UI 设计）
- **Ark UI**（跨框架 headless 组件库，含 Solid 适配）
- 视觉层用 Tailwind 自己拼

代价：拼基础组件（Dialog / Popover / Combobox / Tooltip）的工作量比 shadcn/ui 多。预估 P1 累计 +1-2 周。

### 社区资源是 React 的 1/100

- Stack Overflow / GitHub issue 答案少
- Cursor / Copilot / Claude 对 Solid 的代码补全质量略弱（训练数据少）
- 招聘 / 协作时合作方的学习曲线略高

接受这个代价——投研系统的差异化在性能与功能，而不是 UI 框架的人才广度。

### 心智模型对 React 用户友好

- JSX 语法基本一致
- 组件函数写法相似（但**只跑一次**——signal 变化不重新调用组件函数）
- 大部分 React 经验可迁移：props / children / context / effects

陷阱：React 的 hooks 思维（useState / useEffect）映射到 Solid 时**有微妙差异**——createSignal 不会触发组件重渲染，只触发依赖该 signal 的精确 reactive computation。这个心智切换要文档化（写到 `frontend/tech-stack.md`）。

### 不上 WebAssembly (Leptos / Yew)

WASM 的优势是性能 + 与 Rust gateway 同语言，劣势是：
- 生态极弱：UI 库要自己写，调试 / 热重载 / source map 体验差
- AI 辅助代码生成质量低
- 包体积大，初次加载慢
- DOM 操作要走 `wasm-bindgen` glue，并不比 SolidJS 快

真要 share Rust 逻辑（如某个复杂计算函数），**单点用 `wasm-pack`** 编译 Rust 库 + JS 调用即可，不必整个前端 WASM。

## Alternatives Considered

### React + Vite（被否）
- 优势：生态、shadcn/ui、AI 辅助质量、招聘
- 劣势：高频更新需要补丁；chat-canvas 形态的性能上限受 vdom 拖累
- 否决理由：项目所有者明确"画布上很多高频跳动的金融数字 + 各种图表 + 思维链 DAG，考虑性能空间"——React 的性能补丁路径会持续侵蚀代码质量

### Svelte 5（被否）
- 性能与 SolidJS 相当（runes 是类似 fine-grained reactivity 设计）
- 劣势：与 React 心智差异大（编译时魔法），团队 / AI 辅助迁移成本高
- 生态比 SolidJS 大但 UI 库（如 shadcn-svelte）成熟度也仍在追赶 React 生态

### WebAssembly (Leptos / Yew)（被否，理由见 Consequences §"不上 WebAssembly"）

### Vanilla + Canvas-first（被否）
- 极致性能但 P1 工作量爆炸（自己写状态管理 / 路由 / 表单 / 组件 / 等等）
- 不可接受的 P1 schedule 压力

## 验证标准

- 100 Hz 跳动的 quote 数字 panel × 10 个并存，CPU 使用率 < 30%（M1 Mac）
- 思维链 DAG 100 个节点流式增加 + 动效，60fps 稳定
- 长 session（500 条 message）滚动 / 切换 panel 无明显卡顿
- 初始 bundle 体积（gzip 后）< 200KB（不含 ECharts 等图表库）
