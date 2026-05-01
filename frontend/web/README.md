# L.E.E.K Web (SolidJS + Vite)

L.E.E.K 的生产前端实现。SolidJS（[ADR-0006](../../design/decisions/0006-frontend-solidjs.md)）+ Vite + TypeScript。

## 状态

P1 早期 — 静态视觉稿 port。从 [`design/frontend/prototype/`](../../design/frontend/prototype/) 的 React/babel-standalone 高保真稿移植到 SolidJS，等价 5 个 demo scene（idle / thinking-shallow / clarify / deep / delivered）。**还没接后端**——所有数据是 hardcoded fixture，agent loop / SSE 接入是 M5 的事（详见 [`roadmap.md`](../../design/roadmap.md)）。

## 跑起来

```bash
cd frontend/web
npm install        # 首次
npm run dev        # 起 Vite dev server (port 5173)
npm run typecheck  # 单独跑 TS 检查
npm run build      # 出 dist/
```

切 scene 用 URL hash：`http://localhost:5173/#deep` / `#delivered` / 等等。

## 结构

```
frontend/web/
├── package.json           pnpm 风格的 npm workspace 包
├── vite.config.ts         Vite + vite-plugin-solid
├── tsconfig.json
├── index.html             入口（含 Google Fonts 加载）
└── src/
    ├── index.tsx          SolidJS render entry
    ├── App.tsx            scene picker（dev only）+ Workbench
    ├── styles.css         Claude-warm dark palette + 全部样式（与 prototype 1:1）
    ├── corpus-brain.js    vanilla canvas force-directed graph（与 prototype 1:1）
    ├── scenes.ts          Scene 类型 + 5 个 scene 名
    └── components/
        ├── Icon.tsx           内嵌 SVG 图标
        ├── Panel.tsx          Panel chrome + 11 种 typed module renderer
        ├── Chat.tsx           StreamText / NodeRefPill / Composer / Msg primitives
        ├── Transcripts.tsx    5 个 scene 的 chat transcript
        ├── CanvasScenes.tsx   5 个 scene 的 canvas panel layout
        ├── BrainWidget.tsx    包装 corpus-brain.js 的 SolidJS 组件
        └── Workbench.tsx      TopBar / Rail / CanvasArea / Workbench shell
```

## 与 design fixture 的关系

- [`design/frontend/prototype/`](../../design/frontend/prototype/) 是 React/babel-standalone 的高保真稿（来自 claude.ai/design）——视觉参照标准
- 本目录是 SolidJS 的 1:1 移植——视觉等价，技术栈不同
- prototype 不再维护；视觉迭代都在本目录做

## TODO（M5+）

- 接 SSE / WebSocket 替换 hardcoded scene
- 把 Workbench scene 转成 derived state（基于 task.status / agent loop events）
- DAG 节点容器化（panels.md §4 的 Reasoning DAG canvas 本体）
- Settings page（ProviderConfig + CharterEditor）
- Mention chip 功能化
- 干预流（追加约束 / 中断 → control endpoint）
- corpus-brain 的真实 corpus.graph 数据接入

## 设计 token

完整 token 在 `src/styles.css` 顶部 `:root { --bg-0 ... }`。关键：

- `--bg-0..bg-4`：暖 umber 底层
- `--ink-0..ink-4`：oat 文字
- `--clay`：终端粘土主 accent (`#d97757`)
- `--amber`：琥珀次 accent (`#e8b86c`)
- `--c-prin-wiki / prin-src / know-wiki / know-src`：4 个 corpus tier 颜色
- `--n-quote / chart / fund / corpus / sub / decision`：DAG 节点 typed 颜色
