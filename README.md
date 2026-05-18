# L.E.E.K — Logic-Enhanced Equity Kernel

[English](#english) | [中文](#中文)

---

## English

L.E.E.K (老韭菜) is an AI investment-research agent for retail investors.
Structurally it is a clone of codex / Claude Code — a main agent loop, a tool
registry, built-in skills, a corpus of investing knowledge. What makes it
L.E.E.K is the *content* (a curated investment corpus and domain tools), not
the structure.

### Status — M0 (clean-room skeleton)

The project is being rebuilt **clean-room** on the `rebuild-clean` branch. An
earlier sprint over-engineered the harness; rather than refactor it, the
runtime, migrations and frontend are being rewritten from scratch, one
milestone at a time. Old code is consulted via `git history` only — it is
never carried into the build, route or migration path.

**M0 is the skeleton.** It proves the HTTP + SQLite + SSE plumbing — no agent
loop, no LLM, no OAuth, no tools, no product UI. The gateway accepts sessions
and messages and answers with a fixed `Echo:` reply. M1 wires in the real
agent loop.

What works today:

- **Gateway** (`crates/gateway/`, Rust) — session CRUD, message persistence,
  a per-session event log, and an SSE stream.
- **Echo worker** — every posted message gets an `Echo: <text>` assistant
  reply; each step is persisted as an event and fanned out over SSE
  (`message_created` / `assistant_delta` / `assistant_done`).
- **Harness** (`frontend/web/`, SolidJS) — a minimal chat page for browser
  verification: session list/create, message list, composer, SSE event log.

### Quick start

Requires Rust ≥ 1.85 and Node ≥ 20.

```bash
# build + run the gateway
cargo run -p leek-gateway -- --vault ./vault.db serve --port 8964

# in another shell — the M0 verification harness
cd frontend/web && npm install && npm run dev
# → http://localhost:5173  (proxies /api and /stream to the gateway)
```

Or drive it with `curl` directly — no frontend needed:

```bash
curl localhost:8964/api/v1/health
curl -X POST localhost:8964/api/v1/sessions -H 'content-type: application/json' -d '{"title":"demo"}'
curl -X POST localhost:8964/api/v1/sessions/<id>/messages -H 'content-type: application/json' -d '{"content":"hello"}'
curl localhost:8964/api/v1/sessions/<id>/messages
```

### HTTP API (M0)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v1/health` | Liveness check |
| GET / POST | `/api/v1/sessions` | List / create sessions |
| PATCH / DELETE | `/api/v1/sessions/{id}` | Rename / delete a session |
| GET / POST | `/api/v1/sessions/{id}/messages` | List messages / post a message (runs the echo worker) |
| GET | `/api/v1/sessions/{id}/events?since=&limit=` | Durable event history |
| GET | `/stream/sessions/{id}/events` | Live SSE event stream |

### Repository layout

- `crates/gateway/` — the Rust gateway (HTTP + SQLite + SSE).
- `frontend/web/` — the M0 SolidJS verification harness.
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

### 状态 —— M0（clean-room 骨架）

项目正在 `rebuild-clean` 分支上做 **clean-room 重建**。上一阶段把
harness 做得过度工程化；这次不修补，而是把 runtime、migration、前端
从零重写，逐个 milestone 推进。旧代码只通过 `git history` 查阅 ——
绝不带进编译、路由或 migration 路径。

**M0 是骨架。** 它证明 HTTP + SQLite + SSE 的管路打通 —— 没有 agent
loop、没有 LLM、没有 OAuth、没有工具、没有产品前端。Gateway 接收
session 和 message，用固定的 `Echo:` 回复。真正的 agent loop 在 M1 接入。

当前可用：

- **Gateway**（`crates/gateway/`，Rust）—— session CRUD、消息持久化、
  per-session 事件日志、SSE 流。
- **Echo worker** —— 每条消息得到一个 `Echo: <text>` 回复；每一步都作为
  事件持久化并经 SSE 推送（`message_created` / `assistant_delta` /
  `assistant_done`）。
- **验证 harness**（`frontend/web/`，SolidJS）—— 一个极简 chat 页面，用于
  浏览器端验证：session 列表/创建、消息列表、输入框、SSE 事件日志。

### 快速开始

依赖 Rust ≥ 1.85、Node ≥ 20。

```bash
# 构建并启动 gateway
cargo run -p leek-gateway -- --vault ./vault.db serve --port 8964

# 另开一个终端 —— M0 验证 harness
cd frontend/web && npm install && npm run dev
# → http://localhost:5173（/api 与 /stream 代理到 gateway）
```

也可以直接用 `curl` 驱动，不需要前端：

```bash
curl localhost:8964/api/v1/health
curl -X POST localhost:8964/api/v1/sessions -H 'content-type: application/json' -d '{"title":"demo"}'
curl -X POST localhost:8964/api/v1/sessions/<id>/messages -H 'content-type: application/json' -d '{"content":"hello"}'
curl localhost:8964/api/v1/sessions/<id>/messages
```

### HTTP API（M0）

| 方法 | 路径 | 作用 |
|---|---|---|
| GET | `/api/v1/health` | 存活检查 |
| GET / POST | `/api/v1/sessions` | 列出 / 创建 session |
| PATCH / DELETE | `/api/v1/sessions/{id}` | 重命名 / 删除 session |
| GET / POST | `/api/v1/sessions/{id}/messages` | 列出消息 / 发消息（触发 echo worker） |
| GET | `/api/v1/sessions/{id}/events?since=&limit=` | 事件历史 |
| GET | `/stream/sessions/{id}/events` | SSE 实时事件流 |

### 仓库结构

- `crates/gateway/` —— Rust gateway（HTTP + SQLite + SSE）。
- `frontend/web/` —— M0 SolidJS 验证 harness。
- `docs/ARCHITECTURE.md` —— 端态架构规格。
- `docs/MILESTONES.md` —— milestone 路线图（M0 → M4）与决策日志。
- `AGENTS.md` —— 设计纪律；接手项目先读这个。

项目正在 active rebuild —— A 股纵向（M3）做扎实之前会有大量 churn。
