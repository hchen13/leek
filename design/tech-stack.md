# L.E.E.K Tech Stack

> 项目整体技术栈选型 + 关键 trade-off。大方向决策已经沉淀在 [`decisions/`](decisions/)，本文是按"层"组织的快速 cross-ref。

## 1. Frontend (`frontend/web/`)

| 选型 | 用途 | 为什么不用别的 |
|--|--|--|
| **SolidJS** 1.9+        | UI 框架 | React 的 reconciliation 不适合高频金融数字流；Solid 细粒度 reactive + `createMemo` 适合实时 stream（详见 [ADR-0006](decisions/0006-frontend-solidjs.md)） |
| **Vite** 5+             | Build / Dev server | esbuild + HMR 比 webpack 快 10×；`vite-plugin-solid` 官方维护 |
| **TypeScript** 5.6+     | 类型系统 | `strict` + `isolatedModules` + `allowJs`；后端 Rust 类型可通过 OpenAPI binding 同步 |
| **vanilla CSS**（无 Tailwind / 无 CSS-in-JS） | 样式 | claude design 出的 styles.css 是 Claude-warm dark palette 的视觉参照，按 1:1 移植；Tailwind 会冲淡 craft；CSS-in-JS 在 Solid 没必要 |
| **无 UI 组件库**（无 Kobalte / 无 Radix） | 组件 | claude design 出的所有 component 都是 vanilla DOM；Settings / form 复杂度小，不值得引入；后期需要 modal / popover 时再评估 |
| **vanilla `<canvas>`**（无 d3 / 无 cytoscape）| Force-directed graph | corpus-brain.js 是自实现的 force-directed graph + 激活动画；总 ~480 行；d3 / cytoscape 太重，定制 craft 反而更难 |
| **Solid signal 状态管理**（无 Zustand / 无 Redux） | 状态 | `createSignal` / `createMemo` / `createStore` 够用；scene 是 derived signal（详见 [`frontend/concept.md`](frontend/concept.md) §11） |

### 1.1 关键文件

```
frontend/web/
├── package.json
├── vite.config.ts          Vite + vite-plugin-solid
├── tsconfig.json           strict + allowJs + jsxImportSource=solid-js
├── index.html              入口 + Google Fonts loader
└── src/
    ├── index.tsx           SolidJS render entry
    ├── App.tsx             scene picker (dev only) + Workbench
    ├── styles.css          全部样式（与 prototype 1:1）
    ├── corpus-brain.js     vanilla canvas force graph（与 prototype 1:1）
    ├── scenes.ts           5 scene 名 (demo fixture)
    └── components/
        ├── Icon.tsx
        ├── Panel.tsx           Panel chrome + 11 typed module renderer
        ├── Chat.tsx            StreamText / NodeRefPill / Composer / Msg
        ├── Transcripts.tsx     5 scene 的 chat fixture
        ├── CanvasScenes.tsx    5 scene 的 canvas layout fixture
        ├── BrainWidget.tsx     包装 corpus-brain.js
        └── Workbench.tsx       TopBar / Rail / CanvasArea shell
```

### 1.2 Build 产物

- `npm run build` → `dist/`，~22 KB JS gzip + ~12 KB CSS gzip
- 静态文件，由 Rust gateway 通过 `include_dir!` 编译进 binary，单文件部署

### 1.3 开发期工具

- `npm run dev`：Vite dev server 起 :5173，HMR
- `npm run typecheck`：`tsc --noEmit` 单独跑
- 无 ESLint / Prettier 配置（后期视团队规模决定）

### 1.4 不引入（明确拒绝列表）

- ❌ React / Next.js（与 ADR-0006 冲突）
- ❌ Tailwind CSS（与 vanilla CSS craft 冲突）
- ❌ Zustand / Redux / MobX（Solid signal 已够）
- ❌ Solid Router（Settings / Charter 都是 modal / sub-page；后期视情况引入）
- ❌ shadcn / Radix / Kobalte（没复杂 component 需求）
- ❌ Storybook（component 数量小，snapshot test 即可）
- ❌ d3 / vis.js / cytoscape（自实现 force graph 已够）
- ❌ icon font（用内嵌 SVG 在 Icon.tsx）

## 2. Backend (Rust gateway, P1)

| 选型 | ADR | 用途 |
|--|--|--|
| **Rust** + axum + tokio | [ADR-0001](decisions/0001-rust-gateway.md)              | HTTP / WS / SSE gateway |
| **sqlx** + SQLite (WAL)  | [ADR-0002](decisions/0002-sqlite-vault-single-db.md)  | per-user vault（单库多 user_id） |
| **corpus 静态资源**       | [ADR-0003](decisions/0003-corpus-as-static-resource.md) | git submodule + build-time graph 生成 |
| **不做 ACP**             | [ADR-0004](decisions/0004-no-acp.md)                  | 简化外部 agent 接入 |
| **自实现 harness**        | [ADR-0005](decisions/0005-self-implemented-harness.md) | 不依赖 LangChain；HTTP 直连 LLM provider |
| **事件协议 + 双 transport** | [ADR-0007](decisions/0007-event-protocol-and-transports.md) | SSE 浏览器主用 + WS 外部 agent / 双向 |
| **不做 paper trading**   | [ADR-0008](decisions/0008-no-paper-trading.md)        | 不下单 / 不模拟成交 |
| **portfolio = 投研 context** | [ADR-0009](decisions/0009-portfolio-as-research-context.md) | holdings 是 agent 的输入信号，非交易记录 |
| **single agent + on-demand subagent** | [ADR-0010](decisions/0010-single-agent-coordinator-subagent.md) | map-reduce 模式，无常驻 specialist |

### 2.1 关键依赖

```
gateway crate:
├── axum 0.8           HTTP / WebSocket router
├── tokio 1.47         async runtime
├── sqlx 0.8           SQLite (WAL)
├── reqwest 0.12       HTTP client (LLM API direct)
├── serde / serde_json / serde_yaml
├── async-trait
├── futures
├── uuid v7            time-sortable IDs
├── chrono             ISO8601 timestamps
├── pulldown-cmark     markdown parsing (corpus build)
└── tower-governor     rate limiting
```

### 2.2 LLM provider 实现

详见 [`p1-spec/llm-provider.md`](p1-spec/llm-provider.md)。三个 P1 provider：

- `codex_oauth`（device flow + ChatGPT 订阅复用）
- `anthropic_api_key`（Anthropic Messages API）
- `openai_api_key`（OpenAI Responses API）

全部走 `reqwest` 手写 JSON，**无 SDK**——避免被 SDK 升级节奏拖累。

## 3. Corpus（git submodule `hchen13/the-corpus`）

详见 [`../corpus/AGENTS.md`](../corpus/AGENTS.md)。

- Markdown + YAML frontmatter（Obsidian vault style）
- Wikilinks 形式 `[[wikis/.../page]]`，无 alias
- 维护节奏：与 leek 解耦，独立 git repo
- 工具：`tools/lint.py` / `page_guard.py` / `verify_ingest.py`（Python）

## 4. 部署

详见 [ADR-0001](decisions/0001-rust-gateway.md)：

- 单 binary 部署：Rust gateway 把 `frontend/web/dist/` + `corpus.graph.json` embed 进 binary
- macOS / Linux：`cargo build --release` 出一个可执行文件
- 默认本地启动：`./leek serve --port 8964 --vault ./vault.db --corpus ./corpus`
- Cloud (P2+)：相同 binary 部署到 server，env 配置 `LEEK_AUTH_MODE=token`

## 5. 测试栈

- 后端：`cargo test`（单元 + 集成；mock LLM provider 用 wiremock）
- 前端：M5 接 SSE 后引入 `vitest` + `happy-dom`
- e2e：Playwright（设计 review / regression）

## 6. 不在 P1 范围

| 选型 | 推迟到 |
|--|--|
| Postgres | P2（cloud 多用户切换） |
| Redis | P2（如需 cache / queue） |
| Kubernetes | P2+ |
| 移动 native app | P3+ |
| Multi-agent persistent specialist | P2+（corpus 大到一定程度后） |
| Storybook / 大 e2e suite | M5 之后 |
| ESLint / Prettier 强制 | 视团队规模决定 |
| OpenTelemetry / tracing 全链路 | P2 |
| 国际化（i18n） | P3+，目前 zh-CN single locale |

## 7. 选型变更协议

任何 stack 层面的变更：

1. 写一个新 ADR（`decisions/00XX-<change>.md`）描述动机、影响范围、替代方案、回滚策略
2. 更新本文档对应行，加 reference 到新 ADR
3. 更新 `frontend/web/README.md` 与各 spec 文件中受影响的部分

不允许"先引入再补 ADR"。
