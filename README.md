# L.E.E.K — Logic-Enhanced Equity Kernel

[English](#english) | [中文](#中文)

---

## English

L.E.E.K (老韭菜) is an AI investment-research agent for retail investors.
Structurally it is a clone of codex / Claude Code — a main agent loop, a tool
registry, built-in skills, a corpus of investing knowledge. What makes it
L.E.E.K is the *content* (a curated investment corpus and domain tools), not
the structure.

### Status — M1 (agent loop MVP)

The project is being rebuilt **clean-room** on the `rebuild-clean` branch, one
milestone at a time. Old code is consulted via `git history` only — it is
never carried into the build, route or migration path.

**M0** proved the HTTP + SQLite + SSE plumbing with a fixed `Echo:` reply.
**M1 replaces the echo with a real agent loop.** A posted message now runs the
model–tool cycle on the codex backend, streamed back over SSE, with the full
M1 guard set in place. No corpus, no skills, and no agent delegation yet —
those are later milestones.

What works today:

- **Agent loop** — codex OAuth → Responses API → streamed assistant reply.
  The loop calls the model, dispatches tool calls, feeds results back, and
  repeats until the model finishes or a guard stops it.
- **Tools** — `web_fetch` (fetch a URL as text), `update_plan` (right-rail
  TODO widget), and `corpus_search` / `corpus_read` against the investing
  knowledge layer (M2). Provider-side `web_search` is opt-in via
  `LEEK_WEB_SEARCH`. Domain tools are M3.
- **Guards** — idle timeout, wall-clock ceiling with staged soft-prompts,
  iteration cap, cost cap, and a doom-loop detector. Observability guards
  are on by default; hard caps are opt-in.
- **Auto-compaction** — near the context-window limit a turn folds its
  early context into a traceable summary and continues, instead of
  stopping (M1.8 — replaced the interim context-limit stop).
- **Per-turn metrics** — every turn writes a `turn_metrics` row (stop reason,
  iterations, tokens, cost, triggering guard) and emits a
  `turn_metrics_recorded` event.

### Quick start

Requires Rust ≥ 1.85 and Node ≥ 20.

```bash
# 1. authenticate to the codex backend (one of):
cargo run -p leek-gateway -- --vault ./vault.db auth login    # device flow
cargo run -p leek-gateway -- --vault ./vault.db auth import   # from ~/.codex/auth.json
cargo run -p leek-gateway -- --vault ./vault.db auth status

# 2. run the gateway
cargo run -p leek-gateway -- --vault ./vault.db serve --port 8964

# 3. in another shell — the verification harness
cd frontend/web && npm install && npm run dev
# → http://localhost:5173  (proxies /api and /stream to the gateway)
```

`auth import` copies the codex CLI's current token. Note that leek and the
codex CLI then share one refresh token — whichever refreshes first may
invalidate the other. For a standalone setup, prefer `auth login`.

Or drive it with `curl` — post a message and watch the SSE stream:

```bash
curl localhost:8964/api/v1/health
curl -X POST localhost:8964/api/v1/sessions -H 'content-type: application/json' -d '{"title":"demo"}'
curl -N localhost:8964/stream/sessions/<id>/events &      # watch the turn
curl -X POST localhost:8964/api/v1/sessions/<id>/messages -H 'content-type: application/json' -d '{"content":"hello"}'
```

### HTTP API (M1)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v1/health` | Liveness check |
| GET / POST | `/api/v1/sessions` | List / create sessions |
| PATCH / DELETE | `/api/v1/sessions/{id}` | Rename / delete a session |
| GET | `/api/v1/sessions/{id}/messages` | List messages |
| POST | `/api/v1/sessions/{id}/messages` | Post a message → spawns an agent turn (`202`, returns `turn_id`) |
| GET | `/api/v1/sessions/{id}/events?since=&limit=` | Durable event history |
| GET | `/stream/sessions/{id}/events` | Live SSE event stream |

SSE event kinds (M1.9 workbench contract — each payload carries its
`surface`: `chat` / `canvas` / `right_rail` / `lifecycle`):
`message_created`, `assistant_delta`, `note_trace`, `tool_lifecycle`,
`search_lifecycle`, `plan_updated`, `compaction_started`,
`compaction_completed`, `assistant_done`, `turn_metrics_recorded`,
`error`.

### Guard configuration

Until a settings UI exists, guards are tuned by environment variable
(`serve` reads them at startup):

| Variable | Default | Effect |
|---|---|---|
| `LEEK_IDLE_TIMEOUT_SECS` | `90` | Abort a turn after this much stream silence (`0` disables) |
| `LEEK_WALL_CLOCK_SECS` | `1800` | Hard per-turn deadline; staged soft-prompts in the last 10 min (`0` disables) |
| `LEEK_MAX_ITERATIONS` | off | Cap LLM iterations per turn (opt-in) |
| `LEEK_COST_CAP_USD` | off | Cap estimated USD cost per turn (opt-in) |
| `LEEK_DOOM_LOOP_THRESHOLD` | `3` | Abort after N identical `(tool, args)` calls in a row |
| `LEEK_AUTO_COMPACT_THRESHOLD` | `0.90` | Fraction of the context window at which a turn auto-compacts — summarize early context and continue |
| `LEEK_CONTEXT_WINDOW` | per-model | Override the context window the auto-compaction trigger is sized against, in tokens (mainly for tests — a small window trips compaction within a few turns) |
| `LEEK_WEB_SEARCH` | off | Offer the provider-side `web_search` tool (opt-in; the codex backend gates web search behind its own config) |
| `LEEK_TUSHARE_TOKEN` | off | Token for the primary A-share data source. With no token, only the public-fallback paths work (snapshot quotes via `hq.sinajs.cn`, daily candles via EastMoney); fundamentals + capital-flow tools will return a structured error. Get a free token at <https://tushare.pro/register>. |

### A-share research tools (M3 + M4.1)

L.E.E.K ships eleven vendor-neutral A-share tools. Names and field
shapes are generic (`market_quote`, `revenue`, `roe`, …) — the upstream
identity is hidden from the model and stays in the `vendors/` module.

| Tool | What it does | Vendors (primary → fallback) |
|---|---|---|
| `market_quote` | Snapshot price + day OHLC + volume + freshness | Tushare → Sina |
| `get_candlesticks` | Historical OHLCV bars (1d / 1w / 1mo, ≤500 rows) | Tushare → EastMoney |
| `get_financials` | Income / balance / cashflow / ratios (quarterly or annual) | Tushare |
| `get_company_info` | Company profile + latest valuation indicators | Tushare |
| `get_capital_flow` | Net flow over 1d / 5d / 20d, with northbound when quota allows | Tushare |
| `get_industry_peers` | Same-industry peer set (≤12) on valuation / growth / profitability + target quantile | Tushare |
| `get_business_breakdown` | Main-business revenue split by product / industry / region | Tushare → EastMoney F10 |
| `get_announcements` | Recent announcements + category tag, ≤365 day lookback | Tushare → EastMoney |
| `get_consensus` | Sell-side EPS / net-profit forecasts + rating mix | Tushare → EastMoney |
| `get_top_holders` | Top-10 (total or float) shareholders + QoQ change | Tushare → EastMoney F10 |
| `get_concepts` | Concept / theme tags (≤30) | Tushare → EastMoney |

When every source refuses (no token + fallback denied / out of quota),
the four "supplementary" tools (`industry_peers`, `business_breakdown`,
`consensus`, `top_holders`, `announcements`, `concepts`) return a
structured `data_available: false` payload with a `reason` string — the
model is instructed to surface "data unavailable" instead of fabricating
numbers.

Symbols accept `600519.SH` (preferred), bare `600519` (exchange
inferred), or `sh600519`.

Three subagent profiles wrap these tools into common research shapes
(invoke via the `task` tool):

| Agent | Best for |
|---|---|
| `quick-screen` | "Can I trade X right now?" — 1-2 tool calls, ≈200-300 word digest |
| `deep-review` | Single-stock full review — 10-30 tool calls, 500-1500 word digest with citations |
| `comparison` | N-ticker fan-out — spawns parallel `quick-screen` calls and rolls up to a comparison table |

### Repository layout

- `crates/gateway/` — the Rust gateway (HTTP + SSE + agent loop + vault).
- `frontend/web/` — the SolidJS verification harness.
- `docs/ARCHITECTURE.md` — the end-state architecture spec.
- `docs/MILESTONES.md` — the milestone roadmap (M0 → M4) and decision log.
- `AGENTS.md` — design discipline; read this first when picking up the project.

The project is in active rebuild — expect churn until the A-shares vertical
(M3) is solid.

---

## 中文

L.E.E.K（老韭菜）是一个面向散户的 AI 投研代理。结构上它就是 codex /
Claude Code 的克隆 —— 一个主 agent loop、一套工具注册表、一小撮内置
skill、一份投研 corpus。让它成为 L.E.E.K 的是**内容**（精心维护的投研
corpus 和领域工具），不是结构。

### 状态 —— M1（agent loop MVP）

项目正在 `rebuild-clean` 分支上做 **clean-room 重建**，逐个 milestone
推进。旧代码只通过 `git history` 查阅 —— 绝不带进编译、路由或 migration
路径。

**M0** 用固定的 `Echo:` 回复证明了 HTTP + SQLite + SSE 管路。
**M1 把 echo 换成了真正的 agent loop。** 现在发一条消息会在 codex
后端上跑模型–工具循环，经 SSE 流式回传，并接入完整的 M1 安全网。还没有
corpus、skill、委派子 agent —— 那些是后续 milestone。

当前可用：

- **Agent loop** —— codex OAuth → Responses API → 流式 assistant 回复。
  loop 调模型、派发工具调用、把结果喂回去，循环到模型结束或某个 guard
  中止。
- **工具** —— `web_fetch`（把 URL 抓成文本）、`update_plan`（右栏 TODO
  widget）、以及面向投研知识层的 `corpus_search` / `corpus_read`（M2）。
  Provider 侧 `web_search` 通过 `LEEK_WEB_SEARCH` opt-in。领域工具在 M3。
- **Guards** —— idle timeout、wall-clock 上限（带阶段化软提示）、迭代
  上限、成本上限、doom-loop 检测。可观测性 guard 默认开，硬上限 opt-in。
- **Auto-compaction** —— 上下文接近窗口上限时，turn 把早期上下文折叠成
  可追溯摘要并继续，而不是停下（M1.8 —— 取代了临时的 context-limit 停止）。
- **Per-turn metrics** —— 每个 turn 写一行 `turn_metrics`（停止原因、
  迭代数、token、成本、触发的 guard），并发一个 `turn_metrics_recorded`
  事件。

### 快速开始

依赖 Rust ≥ 1.85、Node ≥ 20。

```bash
# 1. 向 codex 后端认证（二选一）：
cargo run -p leek-gateway -- --vault ./vault.db auth login    # device flow
cargo run -p leek-gateway -- --vault ./vault.db auth import   # 从 ~/.codex/auth.json 导入
cargo run -p leek-gateway -- --vault ./vault.db auth status

# 2. 启动 gateway
cargo run -p leek-gateway -- --vault ./vault.db serve --port 8964

# 3. 另开一个终端 —— 验证 harness
cd frontend/web && npm install && npm run dev
# → http://localhost:5173（/api 与 /stream 代理到 gateway）
```

`auth import` 复制 codex CLI 当前的 token。注意 leek 和 codex CLI 之后会
共用一个 refresh token —— 谁先刷新就可能让对方失效。要做独立部署，优先
用 `auth login`。

也可以用 `curl` 驱动 —— 发一条消息并观察 SSE 流：

```bash
curl localhost:8964/api/v1/health
curl -X POST localhost:8964/api/v1/sessions -H 'content-type: application/json' -d '{"title":"demo"}'
curl -N localhost:8964/stream/sessions/<id>/events &      # 观察这个 turn
curl -X POST localhost:8964/api/v1/sessions/<id>/messages -H 'content-type: application/json' -d '{"content":"你好"}'
```

### HTTP API（M1）

| 方法 | 路径 | 作用 |
|---|---|---|
| GET | `/api/v1/health` | 存活检查 |
| GET / POST | `/api/v1/sessions` | 列出 / 创建 session |
| PATCH / DELETE | `/api/v1/sessions/{id}` | 重命名 / 删除 session |
| GET | `/api/v1/sessions/{id}/messages` | 列出消息 |
| POST | `/api/v1/sessions/{id}/messages` | 发消息 → 启动 agent turn（`202`，返回 `turn_id`） |
| GET | `/api/v1/sessions/{id}/events?since=&limit=` | 事件历史 |
| GET | `/stream/sessions/{id}/events` | SSE 实时事件流 |

SSE 事件类型（M1.9 workbench 契约——每个 payload 带 `surface`：
`chat` / `canvas` / `right_rail` / `lifecycle`）：`message_created`、
`assistant_delta`、`note_trace`、`tool_lifecycle`、`search_lifecycle`、
`plan_updated`、`compaction_started`、`compaction_completed`、
`assistant_done`、`turn_metrics_recorded`、`error`。

### Guard 配置

在设置界面出现之前，guard 通过环境变量调（`serve` 启动时读取）：

| 变量 | 默认 | 作用 |
|---|---|---|
| `LEEK_IDLE_TIMEOUT_SECS` | `90` | 流空闲超过此秒数中止 turn（`0` 关闭） |
| `LEEK_WALL_CLOCK_SECS` | `1800` | 每 turn 硬上限；最后 10 分钟阶段化软提示（`0` 关闭） |
| `LEEK_MAX_ITERATIONS` | 关 | 每 turn 的 LLM 迭代上限（opt-in） |
| `LEEK_COST_CAP_USD` | 关 | 每 turn 估算美元成本上限（opt-in） |
| `LEEK_DOOM_LOOP_THRESHOLD` | `3` | 连续 N 次相同 `(tool, args)` 调用即中止 |
| `LEEK_AUTO_COMPACT_THRESHOLD` | `0.90` | 上下文用量达到窗口的此比例时 turn 自动压缩——摘要早期上下文后继续 |
| `LEEK_CONTEXT_WINDOW` | per-model | 覆盖 auto-compaction 触发线所用的 context window（token 数；主要用于测试——小窗口几个 turn 就触发压缩） |
| `LEEK_WEB_SEARCH` | 关 | 是否提供 provider-side `web_search` 工具（opt-in；codex backend 自身把 web search gate 在其配置后） |
| `LEEK_TUSHARE_TOKEN` | 关 | A 股数据主源 token。不配置时只有公开 fallback 链可用（快照报价走 `hq.sinajs.cn`，日 K 走 EastMoney）；财报和资金流工具会返回结构化错误。免费 token 在 <https://tushare.pro/register> 注册。 |

### A 股投研工具（M3 + M4.1）

L.E.E.K 11 个 A 股工具命名/字段都对厂商中立（`market_quote`、`revenue`、
`roe` 等通用术语）—— 具体上游身份隐藏在 `vendors/` 模块里，不进入模型上下文。

| 工具 | 干什么 | Vendor（主 → fallback） |
|---|---|---|
| `market_quote` | 快照价 + 当日 OHLC + 量能 + freshness | Tushare → Sina |
| `get_candlesticks` | 历史 OHLCV（1d/1w/1mo，≤500 行） | Tushare → EastMoney |
| `get_financials` | 利润表 / 资产负债表 / 现金流 / 比率（季度或年度） | Tushare |
| `get_company_info` | 公司画像 + 最新估值指标 | Tushare |
| `get_capital_flow` | 1d/5d/20d 净流入，北向资金 quota 开通时附带 | Tushare |
| `get_industry_peers` | 同行业可比公司 ≤12 家 + 估值 / 增长 / 盈利分位 | Tushare |
| `get_business_breakdown` | 主营业务收入按 product / industry / region 切分 | Tushare → EastMoney F10 |
| `get_announcements` | 近 365 天公告 + 自动分类（增减持 / 分红 / 解禁…） | Tushare → EastMoney |
| `get_consensus` | 卖方一致预期营收 / 净利 / EPS + 评级分布 | Tushare → EastMoney |
| `get_top_holders` | 前 10 大股东（全部 / 流通）+ 季度变动 | Tushare → EastMoney F10 |
| `get_concepts` | 所属概念 / 题材 ≤30 个 | Tushare → EastMoney |

当所有 vendor 都拒绝（没 token + fallback 也被限）时，6 个 M4.1 工具会
返回结构化的 `data_available: false` + `reason` 字段 —— 模型被指引"明示
不可用"，而**不是**凭印象编数据。这是 leek 的核心数据纪律。

Symbol 接受 `600519.SH`（首选）、bare `600519`（自动推断交易所）、
`sh600519` 任意一种。

三个 subagent 形态把这五个工具组合成常用调研形态（用 `task` 工具调起）：

| Agent | 适用场景 |
|---|---|
| `quick-screen` | "X 现在能不能买" —— 1-2 个工具调用，≈200-300 字 digest |
| `deep-review` | 单只票完整 review —— 10-30 个工具调用，500-1500 字带 cite |
| `comparison` | N 只票横向对比 —— 并行 fan-out `quick-screen` 后做对比表 |

### 仓库结构

- `crates/gateway/` —— Rust gateway（HTTP + SSE + agent loop + vault）。
- `frontend/web/` —— SolidJS 验证 harness。
- `docs/ARCHITECTURE.md` —— 端态架构规格。
- `docs/MILESTONES.md` —— milestone 路线图（M0 → M4）与决策日志。
- `AGENTS.md` —— 设计纪律；接手项目先读这个。

项目正在 active rebuild —— A 股纵向（M3）做扎实之前会有大量 churn。
