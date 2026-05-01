# P1 Spec — Gateway API（HTTP / SSE / WebSocket / MCP）

> Gateway 暴露给所有 client（Web / 外部 agent / MCP client）的完整 API 与事件协议。

依赖：[ADR-0007](../decisions/0007-event-protocol-and-transports.md)（事件协议 + 双 transport）、[`interaction-model.md`](../interaction-model.md)（task lifecycle）、[`agent-loop.md`](agent-loop.md)、[`data-schema.md`](data-schema.md)。

## 1. 顶层结构

```
Gateway HTTP API (默认端口 8964)
├── /api/v1/                          # 主 RESTful API
│   ├── /auth/                        # 认证
│   ├── /sessions/                    # session 管理
│   ├── /tasks/                       # task 操作
│   ├── /messages/                    # 历史 messages 拉取
│   ├── /events/                      # event 历史拉取
│   ├── /artifacts/                   # panel / artifact
│   ├── /vault/                       # vault read API
│   ├── /providers/                   # LLM provider 配置
│   ├── /charter/                     # team charter
│   └── /corpus/                      # corpus 元数据（图、搜索）
├── /stream/                          # 流式 transport
│   ├── /sessions/:id/events (GET)    # SSE 接收事件流
│   └── /sessions/:id/ws    (WS)      # WebSocket 双向
├── /mcp/                             # MCP HTTP transport
│   ├── /sessions/                    # MCP 会话
│   └── /messages/                    # MCP 消息
└── /assets/                          # 前端 SPA 资源（embed in binary）
```

API 版本通过 URL prefix `/api/v1/` 控制；不兼容变更 → `/api/v2/`。

## 2. 认证

P1 起步两种模式：

### 2.1 本地默认（无认证）

启动时 gateway 检查 `LEEK_AUTH_MODE` 环境变量 / 配置项：
- `none`（默认）：所有请求自动认证为 `user_id="local"`
- 适合 local deployment

### 2.2 Token-based（多用户 / cloud 启用时）

```
Authorization: Bearer <session_token>
```

- session_token 通过 `/api/v1/auth/login` 颁发（P1 简化：用户名 + 密码 / 邮箱 + 验证码 / OAuth 等可选其一；细节 P2 拍板）
- token 有效期 7 天，自动刷新
- token 在 server 端存 `vault.auth_sessions`（这张表 P2 添加）

P1 实施只做 `none` 模式。token-based 留接口预留。

### 2.3 用户 ID 解析

每个请求 gateway 内部都要 resolve 出 `user_id`：

```rust
async fn extract_user_id(req: &Request) -> Result<String> {
    match auth_mode {
        AuthMode::None => Ok("local".into()),
        AuthMode::Token => {
            let token = req.header("Authorization")?.strip_prefix("Bearer ")?;
            let session = validate_token(token)?;
            Ok(session.user_id)
        }
    }
}
```

所有后续 API 请求处理都已经知道 `user_id`，不必重复传。

## 3. 通用规范

### 3.1 Request / Response

- Content-Type: `application/json` 默认
- 时间戳全部 ISO 8601 with timezone
- ID 统一用 UUID v7 字符串
- 错误响应统一形态：

```json
{
  "error": {
    "code": "TASK_NOT_FOUND" | "INVALID_INPUT" | "RATE_LIMITED" | ...,
    "message": "human-readable",
    "details": { ... } // optional
  }
}
```

### 3.2 HTTP Status Code 约定

- 200 / 201 — 成功
- 400 — 客户端错误（schema 不匹配、business rule 违反）
- 401 — 未认证
- 403 — 已认证但无权限（如读其他 user 的 task）
- 404 — 资源不存在
- 409 — 冲突（如 task 状态不允许该操作）
- 422 — schema 校验失败
- 429 — rate limited
- 500 — server 错误
- 503 — 服务不可用（如 LLM provider 全部 down）

### 3.3 Pagination

列表 API 统一用 cursor-based pagination：

```
GET /api/v1/tasks?cursor=<token>&limit=20

Response:
{
  "items": [...],
  "next_cursor": "..."  // null = 没有更多
}
```

## 4. RESTful API 详细

### 4.1 Sessions

```
POST /api/v1/sessions
Body: { "title": "周一晨会"? }
Response 201: { "session_id": "..." }

GET /api/v1/sessions
Query: status=active|archived (default: active), pinned=true|false?, limit, cursor
Response: { "items": [{ session 摘要 + open_tasks count }], "next_cursor": "..." }

GET /api/v1/sessions/:id
Response: { full session + tasks list summary }

PATCH /api/v1/sessions/:id
Body: { "title"?, "pinned"?, "status"? }
Response 200

DELETE /api/v1/sessions/:id
Response 204  # 实际是 status=archived（软删除）
```

### 4.2 Tasks

> **前端不直接 POST /tasks**。在 chat-first UX 下，task 由 main agent 从 user message 隐式提取（详见 [`agent-loop.md`](agent-loop.md) §10 first-turn extraction）。本节 endpoints 的实际用途：
> - `POST /tasks`：cron / agent_proposed / 外部 MCP client 等**系统路径**创建 task
> - `POST /tasks/:id/submit`：用户在 TaskBar 接受 proactive task（draft → queued）
> - `POST /tasks/:id/cancel` / `control`：用户对 in_progress task 的干预（TaskBar 上的"中断 / 追加约束"按钮）
> - `GET` / `PATCH`：管理界面查看 / 编辑历史 task
>
> 前端发起新工作的主入口在 §4.3 `POST /sessions/:id/messages`。

```
POST /api/v1/tasks
# 系统路径创建 task（cron / agent_proposed / 外部 MCP client）
Body: {
  "session_id": "...",
  "title": "...",
  "goal": "...",
  "constraints"?: { ... },
  "context_refs"?: ["@NVDA", ...],
  "expected_deliverable": "decision_draft" | ... ,
  "priority"?: "normal",
  "source": "cron" | "agent_proposed" | "mcp_client",
  "submit": true | false  // false = draft（等待用户在 TaskBar 接受）
}
Response 201: { "task_id": "...", "status": "draft" | "queued" }

GET /api/v1/tasks
Query: session_id?, status?, ticker?, limit, cursor
Response: { items, next_cursor }

GET /api/v1/tasks/:id
Response: { full task + deliverable_id + reasoning_dag_summary }

PATCH /api/v1/tasks/:id
Body: { "title"?, "goal"?, "constraints"?, "priority"? }
Conditions:
  · status = draft / queued / in_progress 才允许编辑
  · in_progress 编辑会 emit append_constraint or rescope（详见 control）
Response 200

POST /api/v1/tasks/:id/submit
# 把 draft → queued（用户在 TaskBar 接受 proactive task 时调用）
Response 200

POST /api/v1/tasks/:id/cancel
Response 200

POST /api/v1/tasks/:id/control
Body: ControlCommand
Response 202 (acknowledged async)
```

#### ControlCommand 形态

```typescript
type ControlCommand =
  | { kind: "append_constraint", text: string }
  | { kind: "rescope", new_goal: string, new_constraints: object }
  | { kind: "interrupt" }
  | { kind: "skip_step" }
  | { kind: "pin_panel", panel_id: string }
  | { kind: "user_response", text: string };  // 在 awaiting_user 时
```

Gateway 将 ControlCommand 写入 `vault.events`（kind=control_received），并 push 到对应 task 的 AgentLoop control inbox。

#### Task 状态转换 API

| API / 触发 | 允许的源状态 | 目标状态 |
|--|--|--|
| `POST /sessions/:id/messages` → main agent 第一轮决定开 task | — | `in_progress`（直接进 loop，跳过 queued） |
| `POST /tasks` (submit=true) — cron / agent_proposed | — | `queued` |
| `POST /tasks` (submit=false) — cron / agent_proposed 待用户接受 | — | `draft` |
| `POST /tasks/:id/submit` — 用户在 TaskBar 接受 proactive task | `draft` | `queued` |
| `POST /tasks/:id/cancel` | `queued` / `in_progress` / `awaiting_user` | `cancelled` |
| `POST /tasks/:id/control { kind: "interrupt" }` | `in_progress` | `cancelled`（loop 自然结束） |
| `POST /deliverables/:id/confirm` | task `delivered` | task `confirmed` |
| `POST /deliverables/:id/reject` | task `delivered` | task `rejected` |

### 4.3 Messages

```
GET /api/v1/sessions/:id/messages
Query: task_id?, since_seq?, limit
Response: { items: [{seq, role, content_json, created_at, task_id?}] }

POST /api/v1/sessions/:id/messages
# **前端的主入口**——所有用户在 chat 输入框敲入的内容都从这里进。
# Gateway 接收后路由给该 session 的 main agent loop：
#   · task_id 为空 → main agent 第一轮决定：开新 task / 闲聊回复 / 追加到当前 in_progress task
#   · task_id 非空 → 直接进入指定 task thread（chat 内追问、追加约束等）
# 路由的最终结果通过 SSE / WS 的事件流揭晓（task_created / agent_message_start / 等）
Body: {
  "task_id"?: "...",        # 可选；缺省 = 让 main agent 自己判断（详见 agent-loop.md §10）
  "content": { "type": "text", "text": "..." }
}
Response 201: { "message_seq": number }
```

详细的 first-turn extraction 协议（main agent 如何从 user message 决定路由）见 [`agent-loop.md`](agent-loop.md) §10。

### 4.4 Events（历史 / 续传）

```
GET /api/v1/sessions/:id/events
Query: task_id?, since_seq?, kinds?[], limit
Response: { items: [{seq, task_id, kind, payload_json, ts}], next_cursor }
```

主要用于：
- 重新打开 session 时重建 UI 状态
- SSE / WebSocket 断线重连时拉缺失事件（用 `since_seq` = Last-Event-ID）

### 4.5 Artifacts

```
GET /api/v1/sessions/:id/artifacts
Query: task_id?, kind?, pinned?, limit, cursor
Response: { items, next_cursor }

GET /api/v1/artifacts/:id
Response: full artifact

PATCH /api/v1/artifacts/:id
Body: { "pinned"?: true|false }
Response 200
```

### 4.6 Deliverables

```
GET /api/v1/tasks/:id/deliverables
Response: { items: [...] }

GET /api/v1/deliverables/:id
Response: full deliverable

POST /api/v1/deliverables/:id/confirm
Body: { "edits"?: { ... } }   # 用户在 confirm 前对 deliverable 做的最终编辑
Response 200
# 副作用：
#   · deliverables.status = "confirmed"
#   · 派生写入：decision_draft → vault.decisions; review → vault.reviews
#   · task.status = "confirmed"
#   · emit Event::DeliverableConfirmed

POST /api/v1/deliverables/:id/reject
Body: { "reason"?: "..." }
Response 200
# 副作用：
#   · deliverables.status = "rejected"
#   · task.status = "rejected"
#   · emit Event::DeliverableRejected

POST /api/v1/deliverables/:id/respawn
Body: { "new_goal"?, "new_constraints"? }
Response 201: { "new_task_id": "..." }
# 基于本 deliverable 创建新 task，带原 task 的 context
```

### 4.7 Vault read

```
GET /api/v1/vault/holdings
Query: snapshot_at?  # 缺省返回最新
Response: { snapshot_at, holdings: [...], summary: {...} }

POST /api/v1/vault/holdings
Body: { snapshot_at?, holdings: [{ticker, qty, avg_cost?, notes?, account?}], replace_full: true|false }
Response 201
# replace_full=true 表示传的是完整快照（覆盖式）；false 表示部分更新

POST /api/v1/vault/holdings/import
# multipart/form-data: csv file
Response 201: { snapshot_at, parsed: [...], skipped_rows: [...] }

GET /api/v1/vault/decisions
Query: ticker?, status?, since?, limit, cursor
Response: { items, next_cursor }

GET /api/v1/vault/decisions/:id
Response: full decision

GET /api/v1/vault/reviews
Query: decision_id?, since?, limit, cursor
Response: { items, next_cursor }

GET /api/v1/vault/reviews/:id
Response: full review

GET /api/v1/vault/watchlists
Response: [{id, name, tickers, sort_order}]

POST /api/v1/vault/watchlists
Body: { name, tickers }
Response 201

PATCH /api/v1/vault/watchlists/:id
Body: { name?, tickers?, sort_order? }
Response 200

DELETE /api/v1/vault/watchlists/:id
Response 204
```

### 4.8 LLM Providers

```
GET /api/v1/providers
Response: [
  {
    "name": "codex_oauth",
    "auth_kind": "oauth",
    "status": "active" | "disabled" | "invalid",
    "default_model": "gpt-5",
    "model_aliases": {...},
    "priority": 100,
    "last_used_at": "...",
    "last_error": "...",
    # OAuth 特有：
    "oauth_account_email": "...",
    "oauth_expires_at": "...",
  },
  ...
]

POST /api/v1/providers/:name/configure
Body (API key): { "auth_kind": "api_key", "api_key": "...", "default_model": "...", "model_aliases": {...} }
Body (OAuth): { "auth_kind": "oauth" }   # 触发 device flow
Response:
  API key path: 200 (immediate)
  OAuth path: 202 + { "device_flow": { "user_code": "ABCD-1234", "verification_uri_complete": "...", "polling_endpoint": "/api/v1/providers/codex_oauth/oauth_status?flow_id=...", "expires_in": 900 } }

GET /api/v1/providers/:name/oauth_status?flow_id=...
Response: { "status": "pending" | "authorized" | "expired" | "denied", ... }
# Long-polling 或 short-polling，浏览器侧每 5s 调一次

POST /api/v1/providers/:name/test
Response: { "ok": true | false, "duration_ms": ..., "error"?: "..." }

PATCH /api/v1/providers/:name
Body: { "enabled"?, "priority"?, "default_model"?, "model_aliases"? }
Response 200

DELETE /api/v1/providers/:name
Response 204

POST /api/v1/providers/chain
Body: { "chain": ["codex_oauth", "anthropic_api_key", "openai_api_key"] }  # priority 顺序
Response 200
```

### 4.9 Team Charter

```
GET /api/v1/charter
Response: { id, version, charter_json, updated_at }

PUT /api/v1/charter
Body: { charter_json: {...} }
Response: { id, version, updated_at }
# 创建一个新 active charter（旧版本保留为非 active）

GET /api/v1/charter/history
Response: [{id, version, charter_json, updated_at}, ...]
```

### 4.10 Corpus

```
GET /api/v1/corpus/graph
Query: cluster?[]
Response: { nodes: [...], edges: [...] }

GET /api/v1/corpus/search
Query: q, cluster?, limit
Response: { items: [{wikilink_id, title, excerpt, score}, ...] }

GET /api/v1/corpus/read?wikilink_id=...
Response: { wikilink_id, title, frontmatter, content_md, related: [...] }

POST /api/v1/corpus/reload
Response 200
# 手动触发 corpus 重新扫描（如果文件改了）
```

### 4.11 系统 / 健康

```
GET /api/v1/health
Response: { ok: true, version: "...", uptime_sec, vault_writable: true, corpus_loaded: true }

GET /api/v1/usage
Query: since?, until?
Response: { providers: [{ provider_name, input_tokens, output_tokens, calls }, ...] }
```

## 5. 流式 Transport

### 5.1 SSE 端点（浏览器主用）

```
GET /stream/sessions/:id/events
Headers:
  Accept: text/event-stream
  Last-Event-ID: <last_seq>?    # 续传

Response: text/event-stream
  event: <kind>
  id: <seq>
  data: <json payload>
  
  # 心跳每 30s 一次
  event: heartbeat
  id: <seq>
  data: {"ts": "..."}
```

行为：
- 连接建立时 gateway 立即推送过去 100 条 events 帮 client 重建 UI 状态
- 之后每个 EventBus 推送的事件实时推过来
- 断线时浏览器 EventSource 自动重连，带 `Last-Event-ID` → gateway 从 vault 拉缺失事件续传
- **不接收** client 数据——所有 client → server 走 HTTP POST

### 5.2 WebSocket 端点（外部 agent / 双向）

```
WS /stream/sessions/:id/ws
```

#### Client → Server frames

```typescript
type ClientFrame =
  | { action: "subscribe", session_id: string, since_seq?: number }
  | { action: "unsubscribe" }
  | { action: "send_message", task_id: string, content: ContentPart }
  | { action: "control", task_id: string, command: ControlCommand }
  | { action: "submit_task", task: TaskDraft }
  | { action: "ping" }
  | { action: "resume", from_seq: number };  // 断线重连时跳过已收到事件
```

#### Server → Client frames

```typescript
type ServerFrame =
  | { kind: "event", event: Event }    // 与 SSE 同 schema 的事件
  | { kind: "ack", request_id: string, status: "ok" | "error", error?: string }
  | { kind: "pong" }
  | { kind: "error", error: string };
```

行为：
- 连接建立后 client 发 `subscribe`（含 since_seq 续传）
- Gateway 推送所有 EventBus 的事件（与 SSE 同源）
- Client 可以发 `send_message` / `control` / `submit_task` 等反向操作（这些操作背后还是 RESTful 的等效行为，但走 WS 减少 HTTP 开销）
- 心跳：client 每 25s 发 ping；server 5s 内回 pong；超时 → 断开重连

## 6. 事件协议

### 6.1 Envelope

```typescript
type Event = {
  session_id: string;
  user_id: string;
  task_id?: string;          // 大部分事件有 task_id
  seq: number;               // per-session 单调递增
  kind: EventKind;
  payload: object;           // kind-specific schema
  ts: string;                // ISO8601 high-precision
  source?: string;           // "main_agent" | "subagent:<run_id>" | "user" | "system"
};
```

### 6.2 EventKind 枚举（完整）

```typescript
type EventKind =
  // ────── Session / Task lifecycle ──────
  | "session_created"
  | "session_archived"
  | "task_created"
  | "task_queued"
  | "task_started"
  | "task_status_changed"
  | "task_delivered"
  | "task_confirmed"
  | "task_rejected"
  | "task_cancelled"
  | "task_failed"
  | "task_constraints_updated"
  | "task_rescoped"

  // ────── User input / Chat ──────
  | "user_message"
  | "user_message_in_thread"

  // ────── Agent thinking / messaging ──────
  | "agent_thinking_start"
  | "agent_thinking_delta"
  | "agent_message_start"
  | "agent_message_delta"
  | "agent_message_end"

  // ────── Tool calls ──────
  | "tool_call_detected"
  | "tool_call_args_delta"
  | "tool_call_start"
  | "tool_call_result"

  // ────── Subagent ──────
  | "subagent_started"
  | "subagent_progress"
  | "subagent_completed"

  // ────── Reasoning DAG ──────
  | "reasoning_dag_node"
  | "reasoning_dag_edge"
  | "reasoning_dag_pruned"      // context 裁剪时

  // ────── Corpus 激活 ──────
  | "corpus_node_activated"

  // ────── Panels ──────
  | "panel_open"
  | "panel_update"
  | "panel_close"
  | "panel_pinned"

  // ────── Deliverable ──────
  | "deliverable_draft_started"
  | "deliverable_draft_updated"
  | "deliverable_ready"
  | "deliverable_confirmed"
  | "deliverable_rejected"

  // ────── Clarification ──────
  | "clarification_requested"
  | "clarification_answered"

  // ────── Control ──────
  | "control_received"
  | "control_ack"

  // ────── Errors ──────
  | "error"
  | "warning"

  // ────── LLM resource ──────
  | "llm_usage"
  | "llm_provider_changed"     // 降级到 fallback 时

  // ────── Heartbeat ──────
  | "heartbeat";
```

### 6.3 Payload Schema（关键事件）

#### `task_started`

```json
{
  "task_id": "...",
  "title": "...",
  "goal": "...",
  "expected_deliverable": "decision_draft"
}
```

#### `agent_thinking_delta`

```json
{
  "text": "Let me check the portfolio first..."
}
```

#### `agent_message_delta`

```json
{
  "text": "Based on the analysis"
}
```

#### `tool_call_detected`

```json
{
  "id": "toolu_abc",
  "name": "quote.get"
}
```

#### `tool_call_args_delta`

```json
{
  "id": "toolu_abc",
  "delta": "{\"ticker\":"
}
```

#### `tool_call_start`

```json
{
  "run_id": "...",
  "tool_call_id": "toolu_abc",
  "name": "quote.get",
  "arguments": { "ticker": "NVDA" }
}
```

#### `tool_call_result`

```json
{
  "run_id": "...",
  "tool_call_id": "toolu_abc",
  "success": true,
  "result": { ... },
  "duration_ms": 230
}
```

#### `subagent_started`

```json
{
  "run_id": "subagent_run_uuid",
  "spec_name": "valuation_dcf",
  "scope": { ... },
  "input_summary": "评估 NVDA 的 DCF 估值..."
}
```

#### `subagent_progress`

```json
{
  "run_id": "...",
  "turn": 2,
  "tokens_used": 1840,
  "current_action": "calling tool: financials.history"
}
```

#### `subagent_completed`

```json
{
  "run_id": "...",
  "success": true,
  "result": { ... },
  "summary": "DCF 估值结果 $520...",
  "tokens_used": 4200,
  "turns": 3,
  "duration_ms": 12300
}
```

#### `reasoning_dag_node`

```json
{
  "node_id": "...",
  "kind": "tool_call" | "thinking" | "observation" | "corpus_ref" | ...,
  "title": "查询 NVDA 实时报价",
  "details": "...",
  "subagent_run_id": "..."?,
  "ts": "..."
}
```

#### `reasoning_dag_edge`

```json
{
  "edge_id": "...",
  "from": "node_id_1",
  "to": "node_id_2"
}
```

#### `corpus_node_activated`

```json
{
  "wikilink_id": "principles/margin-of-safety",
  "intensity": "search_hit" | "deep_read" | "cited",
  "trigger_tool_call_id": "..."
}
```

#### `panel_open`

```json
{
  "panel_id": "...",
  "kind": "quote" | "chart" | "decision_draft" | ...,
  "payload": { ... },
  "layout_hint": { "size": "M" }
}
```

#### `panel_update`

```json
{
  "panel_id": "...",
  "patch": { ... },          // JSON Merge Patch (RFC 7396)
  "version": 7
}
```

#### `clarification_requested`

```json
{
  "task_id": "...",
  "question": "你说的 NVDA 是 Nvidia 还是 Navidec？",
  "options": ["Nvidia", "Navidec", "其他"]?,
  "why": "搜索到两个匹配的 ticker"?
}
```

#### `deliverable_ready`

```json
{
  "deliverable_id": "...",
  "task_id": "...",
  "kind": "decision_draft",
  "summary": "建议 NVDA 加仓 15 股，止损 $440"
}
```

#### `llm_usage`

```json
{
  "provider": "codex_oauth",
  "model": "gpt-5",
  "input_tokens": 4200,
  "output_tokens": 1100,
  "cache_read_tokens": 3500,
  "cache_write_tokens": 0,
  "duration_ms": 1820
}
```

### 6.4 事件协议版本化

`/api/v1/` URL prefix 决定 RESTful API 的版本。事件协议本身在 envelope 顶层不显式带 version——每个 EventKind 的 payload schema 演化遵循：

- **加字段** = backward-compatible，不需要新版本
- **删字段** = 先 deprecate 一个版本周期再删
- **改字段类型** = 新 EventKind（如 `tool_call_result_v2`），老 kind 保留并 deprecate

P1 启动时所有 EventKind 都是 v1（不显式带后缀）。

### 6.5 Last-Event-ID 续传

每个 SSE event 带 `id: <seq>` 字段；`seq` 是 per-session 单调递增（与 messages.seq 不共享，单独计数）。

WebSocket 等效于 client 发 `{"action": "resume", "from_seq": N}` 帧。

Gateway 内部把每个 session 的最近 100 条事件保留在内存（环形 buffer），更早的从 vault 拉。

## 7. MCP HTTP 接入

L.E.E.K 同时实现 MCP server (HTTP transport)，暴露**leek 的工具子集**给外部 agent。

### 7.1 端点

```
POST /mcp/sessions
# Initialize MCP session
Body: MCP InitializeRequest
Response: MCP InitializeResponse + session_id

POST /mcp/messages?session_id=...
# Send MCP messages (tools/list / tools/call / resources/list / etc)
Body: MCP message (JSON-RPC 2.0)
Response: streamable-http (HTTP POST + SSE for streaming results)
```

具体 MCP 协议遵循 spec 不在此重复。

### 7.2 暴露的工具

P1 暴露给 MCP client 的工具子集（read-only 为主）：

- `corpus.search`
- `corpus.read`
- `vault.holdings.current`
- `vault.decisions.list`
- `vault.decisions.get`
- `vault.charter.get`
- `quote.get`（选可暴露）
- `chart.ohlc`（选可暴露）

**不暴露**：所有 write 工具（`decision.draft` / `holdings.update` / `panel.*` / `subagent.spawn`）—— 这些只能通过 leek 自己的 chat-canvas 触发，避免外部 agent 写脏 vault。

未来可以加 "我把 leek 当成 agent 委托长任务" 的入口，但那是通过 leek 自己的 RESTful + WebSocket，不是 MCP。

## 8. CORS / 安全

P1 默认：

- CORS 白名单：`http://localhost:*`（允许任意 localhost 端口）
- 生产部署（cloud）：用户配置允许的 origin
- CSRF 防护：`/api/v1/*` 的 mutating endpoints 必须带 `Origin` header 在白名单内

API key / OAuth token 等敏感数据：
- 永远不进 log（logging middleware 自动 mask）
- 不进 metrics
- 不在 error response 里 echo 回来

## 9. 限流

P1 简化：

- 单 user 全 endpoint 总 QPS：50
- 单 user `/tasks` POST QPS：5（防止任务爆炸）
- 单 user `/messages` POST QPS：20

实现：tower-governor 中间件，每 endpoint 独立限流策略。

## 10. 错误码清单

```typescript
type ErrorCode =
  | "AUTH_REQUIRED"
  | "AUTH_INVALID"
  | "PERMISSION_DENIED"
  | "NOT_FOUND"
  | "INVALID_INPUT"
  | "SCHEMA_VALIDATION_FAILED"
  | "TASK_STATUS_CONFLICT"          // task 当前状态不允许该操作
  | "DELIVERABLE_NOT_READY"
  | "PROVIDER_UNAVAILABLE"          // 所有 LLM provider 失败
  | "QUOTA_EXCEEDED"
  | "CORPUS_NOT_LOADED"
  | "VAULT_WRITE_FAILED"
  | "RATE_LIMITED"
  | "INTERNAL_ERROR";
```

## 11. 实施 checklist

- [ ] axum router 骨架 + 中间件链（auth / cors / rate limit / logging）
- [ ] RESTful endpoints 每个组（sessions / tasks / messages / events / artifacts / deliverables / vault / providers / charter / corpus / health / usage）
- [ ] SSE endpoint（含 Last-Event-ID 续传）
- [ ] WebSocket endpoint（含 frame schema + heartbeat）
- [ ] EventBus + per-session subscription
- [ ] 事件持久化（异步 batch 写 vault.events）
- [ ] MCP HTTP server（streamable-http transport）
- [ ] OpenAPI / TypeScript binding 自动生成（前端共用类型）
- [ ] 单元测试每个 endpoint
- [ ] 集成测试：完整 task lifecycle 通过 HTTP API + SSE
- [ ] e2e 测试：浏览器端 + 模拟外部 agent 同时连一个 session
