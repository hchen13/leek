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
- **Tools** — `echo` (deterministic, proves the function-call loop) and
  `web_fetch` (fetch a URL as text). Domain tools are M3.
- **Guards** — idle timeout, wall-clock ceiling with staged soft-prompts,
  iteration cap, cost cap, doom-loop detector, and a context-limit threshold
  guard. Observability guards are on by default; hard caps are opt-in.
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

SSE event kinds: `message_created`, `assistant_delta`, `tool_call`,
`tool_result`, `assistant_done`, `turn_metrics_recorded`, `error`.

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
| `LEEK_AUTO_COMPACT_THRESHOLD` | `0.90` | Stop with a diagnostic when context reaches this fraction of the window |

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
- **工具** —— `echo`（确定性，验证 function-call 循环）和 `web_fetch`
  （把 URL 抓成文本）。领域工具在 M3。
- **Guards** —— idle timeout、wall-clock 上限（带阶段化软提示）、迭代
  上限、成本上限、doom-loop 检测、context-limit 阈值 guard。可观测性
  guard 默认开，硬上限 opt-in。
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

SSE 事件类型：`message_created`、`assistant_delta`、`tool_call`、
`tool_result`、`assistant_done`、`turn_metrics_recorded`、`error`。

### Guard 配置

在设置界面出现之前，guard 通过环境变量调（`serve` 启动时读取）：

| 变量 | 默认 | 作用 |
|---|---|---|
| `LEEK_IDLE_TIMEOUT_SECS` | `90` | 流空闲超过此秒数中止 turn（`0` 关闭） |
| `LEEK_WALL_CLOCK_SECS` | `1800` | 每 turn 硬上限；最后 10 分钟阶段化软提示（`0` 关闭） |
| `LEEK_MAX_ITERATIONS` | 关 | 每 turn 的 LLM 迭代上限（opt-in） |
| `LEEK_COST_CAP_USD` | 关 | 每 turn 估算美元成本上限（opt-in） |
| `LEEK_DOOM_LOOP_THRESHOLD` | `3` | 连续 N 次相同 `(tool, args)` 调用即中止 |
| `LEEK_AUTO_COMPACT_THRESHOLD` | `0.90` | 上下文达到窗口此比例时诊断性停止 turn |

### 仓库结构

- `crates/gateway/` —— Rust gateway（HTTP + SSE + agent loop + vault）。
- `frontend/web/` —— SolidJS 验证 harness。
- `docs/ARCHITECTURE.md` —— 端态架构规格。
- `docs/MILESTONES.md` —— milestone 路线图（M0 → M4）与决策日志。
- `AGENTS.md` —— 设计纪律；接手项目先读这个。

项目正在 active rebuild —— A 股纵向（M3）做扎实之前会有大量 churn。
