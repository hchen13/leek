# ADR 0007 — 事件协议统一 + SSE / WebSocket 双 transport

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0001](0001-rust-gateway.md)（gateway 是事件源）、[0006](0006-frontend-solidjs.md)（前端消费事件流）

## Context

L.E.E.K 的客户端有两种主要消费形态：

1. **浏览器（人主用）**：发送消息后等待 agent 流式响应（thinking / tool calls / panel updates），通常**单向、短任务**——用户在线时交互
2. **外部 agent（agent 间调用）**：委托长任务给 leek，自己端要展示进度 / 工具调用 / 中间结果；甚至可能用户离线后异步通知。包括**双向**：外部 agent 中途可能要发指令（"打断这一轮"、"切换关注的标的"）

这两种场景在 transport 层有不同需求：

| 场景 | 接收 | 反向通道 | 离线 / 异步通知 |
|--|--|--|--|
| 浏览器 chat | 流式接收 | 一般每次都新 HTTP POST 即可 | 不必（用户在线） |
| 外部 agent 长任务 | 流式接收（要订阅） | 中途要能发 control 指令 | 需要（可能等很久） |

候选 transport 方案：

| 方案 | 浏览器 | 外部 agent 长任务 | 内部架构复杂度 |
|--|--|--|--|
| **A. SSE 主轴 + HTTP POST 反向** | ✓ 简单 | 反向通道每次新 POST，长任务不优雅 | 低 |
| **B. WebSocket 主轴** | 浏览器调试稍麻烦（DevTools 看 WS frame 不如 SSE event） | ✓ 完美 | 中 |
| **C. SSE 给浏览器 + WebSocket 给外部 agent，事件协议统一** | ✓ 简单 | ✓ 完美 | 中（一份事件源 + 两个 transport adapter） |
| **D. 各 transport 各自定义事件格式** | — | — | 高（事件协议碎片化） |

## Decision

**采用方案 C：事件协议统一 + SSE / WebSocket 双 transport。**

- 浏览器默认 **SSE**（`GET /sessions/:id/events`），提交走 HTTP POST
- 外部 agent 默认 **WebSocket**（`WS /sessions/:id/ws`，双向）
- MCP HTTP client 走 **streamable-http**（MCP 协议自带，是 HTTP POST + SSE）
- **Gateway 内部一份 EventBus**，所有事件源（agent loop / tool runner / persistence）发到 bus，所有 transport adapter 订阅 bus 各自序列化推给 client

### 统一事件协议

```jsonc
{
  "session_id": "uuid",
  "user_id": "local",
  "kind": "agent_thinking" | "agent_message_delta" | "tool_call_start"
        | "tool_call_args_delta" | "tool_call_result" | "panel_open"
        | "panel_update" | "panel_close" | "reasoning_dag_node"
        | "reasoning_dag_edge" | "agent_done" | "error" | "control_ack",
  "payload": { /* kind-specific schema */ },
  "ts": "2026-05-01T14:30:00.123Z"
}
```

具体每个 `kind` 的 payload schema 在 `p1-spec/api.md`（待写）里定义。

### 反向通道（client → gateway）的语义

| Client | 反向操作 | 协议形式 |
|--|--|--|
| 浏览器 | 发送 user message | `POST /sessions/:id/messages` |
| 浏览器 | 打开 panel / 关闭 panel | `POST /sessions/:id/actions` |
| 浏览器 | 中断当前 turn | `POST /sessions/:id/interrupt` |
| 外部 agent | 发送 user message | WS 帧 `{"action": "send_message", ...}` |
| 外部 agent | 中断 / 切换上下文 | WS 帧 `{"action": "interrupt"}` / `{"action": "set_context", ...}` |

浏览器所有反向操作都是单独的 HTTP POST，**不强制双向 WS**——后续如果浏览器要支持"用户语音中断 agent"或"协作模式"，无缝升级到 WebSocket，**事件协议不变**。

### Gateway 内部架构

```
                        ┌─────────────────────────────────┐
                        │ Agent Loop                      │
                        │  · LLM stream                   │
                        │  · Tool dispatcher              │
                        │  · Panel state machine          │
                        └────────────────┬────────────────┘
                                         │ Event { session_id, kind, payload }
                                         ▼
                        ┌─────────────────────────────────┐
                        │ EventBus                        │
                        │  · per-session pub/sub          │
                        │  · 持久化所有 event 到 vault    │
                        └────────────────┬────────────────┘
                                         │
              ┌──────────────────┬──────-+──────────┬──────────────────┐
              ▼                  ▼                  ▼                  ▼
        ┌─────────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
        │ SSE Adapter │    │ WS Adapter  │    │ MCP HTTP    │    │ Persistence │
        │             │    │             │    │ Adapter     │    │             │
        │ → 浏览器     │    │ → 外部 agent │    │ → MCP client│    │ → vault     │
        └─────────────┘    └─────────────┘    └─────────────┘    └─────────────┘
```

每个 transport adapter 持有自己的 client 连接表，订阅 EventBus，按各自协议格式序列化发出去。**事件源 → EventBus → 多 adapter** 是单向 fan-out。

## Consequences

### 浏览器场景实现简单

- 提交：标准 `POST` form / JSON
- 接收：`new EventSource('/sessions/xxx/events')`，浏览器原生支持，自带断线重连、Last-Event-ID 续传
- 不依赖 WebSocket 库
- DevTools 直接看 SSE event stream，调试友好

### 外部 agent 场景全功能

- 双向打断 / 切换上下文 / control 指令通过 WS 帧
- 长任务可以"提交后断开" + "后续重连订阅"——session_id 是稳定的，重连后 EventBus 把缺失事件续推（结合 Last-Event-ID）
- 异步通知场景（任务跑完用户已离线）：所有事件持久化到 vault.artifacts，下次连上时 client 主动 `GET /sessions/:id/events?from=<ts>` 拉取

### Last-Event-ID 续传

每个事件带 `seq` 字段（per-session 单调递增）。SSE 用 Last-Event-ID 头部，WebSocket 用 `{"action": "resume", "from_seq": N}` 帧。Gateway 内部把每个 session 的最近 N 条事件保留在内存（默认 100 条），更早的从 vault 读。

### 浏览器升级路径无痛

如果浏览器后续要支持双向（语音中断、协作模式）：

- 复用同一份事件协议，换 transport 实现
- 后端改动：浏览器 endpoint 从 `GET /events` (SSE) 加一个 `GET /ws` (WebSocket) 入口
- 前端改动：从 EventSource 切到 WebSocket client
- 事件 envelope 不变，业务代码不改

### 事件协议是 contract，所有变化要 versioned

- 顶层 envelope（session_id / user_id / kind / payload / ts / seq）相对稳定
- payload 内部的 schema 演化要 backward compatible（加字段不删字段，删字段先 deprecate）
- 重大破坏性变化通过 `event_protocol_version` 协议头协商
- 详见 `p1-spec/api.md`

### 持久化所有事件

- 每个事件都进 `vault.artifacts` (kind=event)，可以重放整个 session
- 长 session 的存储成本：每条 message 5-50 个事件，500 条 message session 存 ~25k 行——SQLite 完全 OK
- 可以异步 batch 写入降低延迟（攒 50 个 event 或 1s timeout）

## Alternatives Considered

### A. SSE 主轴 + HTTP POST 反向（被否）
- 浏览器场景完美但外部 agent 长任务的反向控制（中途打断 / 切换上下文）每次都新 HTTP POST 不优雅
- 长连接的 health check 在外部 agent 离线 / 重连场景下做不优雅

### B. WebSocket 主轴（被否）
- 浏览器场景过度设计，调试也不如 SSE 友好
- 浏览器 EventSource 自带的断线重连 / Last-Event-ID 都要在 WS 层重新实现
- 不必要的复杂度

### D. 各 transport 各自定义事件（被否）
- 事件 schema 碎片化，每加一个 transport 要写一份新协议文档
- 持久化层无法统一处理事件
- agent loop 端要为不同 transport 写不同的事件发送代码

## 验证标准

- SSE 主链路：浏览器 → gateway p95 事件延迟 < 50ms（不含 LLM 推理）
- WebSocket 主链路：外部 agent → gateway 双向 RTT < 20ms（local），打断指令 200ms 内生效
- 断线重连：网络中断 30s 后重连，SSE / WebSocket 都能续传未消费事件，0 丢失
- 单 gateway 进程支持 100 个并发 session，每个 session 平均 5Hz 事件，CPU < 50%（M1 Mac）
