# L.E.E.K 系统架构

> Logic-Enhanced Equity Kernel —— 投研操作系统的整体架构总览。本文档是 L.E.E.K 设计的 root document，所有更细的 spec / ADR / frontend 设计都从这里 fan-out。

## 1. 一句话定位

L.E.E.K 是一个 **gateway 模式的投研操作系统**：长跑 daemon 自带 agent 思考能力，把一份策划过的投资智慧 corpus 转化成可执行的研究、决策与复盘，通过多种 adapter（Web / CLI / MCP HTTP / 未来 TUI / Claude Code skill）暴露给人和其他 agent。

**它不是什么**：
- 不是一个 agent —— 是 agent core + 数据双核 + 多 frontend 的系统
- 不是 corpus —— corpus 是它消费的静态资源，住在 sibling 仓库 `~/playground/finance-giant/corpus/`
- 不是另一个 agent harness 的 frontend —— L.E.E.K 自己实现 agent loop，自己持有 LLM provider 抽象，不把思考 delegate 给 Claude Code / Codex
- 不是模拟交易系统 —— 投资动作的输出是决策草稿（仓位 / 止损 / 期限 / 复盘 schedule），由人最终落地

## 2. 整体架构图

```
┌──────────────────────────────────────────────────────────────────┐
│ Adapters（多种入口，连同一个核心）                                │
│                                                                  │
│  人直接用：    Web (chat-canvas, SolidJS) | (P2) TUI | (P2) CLI │
│  Agent 间调:  MCP HTTP | (P2) Claude Code skill                  │
└─────────────────────────┬────────────────────────────────────────┘
                          │  HTTP POST + SSE / WebSocket
                          │  事件协议统一（详见 ADR-0007）
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│ Gateway (Rust 长跑 daemon)                                       │
│                                                                  │
│  ┌──────────────────────┐  ┌──────────────────────────────────┐  │
│  │ Transport Layer      │  │ Session / Auth / User Scope      │  │
│  │ axum + tokio         │  │ user_id day-1                    │  │
│  │ HTTP / SSE / WS      │  │                                  │  │
│  └──────────────────────┘  └──────────────────────────────────┘  │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │ Agent Core (自实现 harness)                              │    │
│  │  · Loop (think → tool_call → observe → reply)            │    │
│  │  · Scratchpad (per-session)                              │    │
│  │  · Thinking traces                                       │    │
│  │  · Multi-turn conversation context                       │    │
│  └──────────────┬─────────────────┬────────────────┬────────┘    │
│                 │                 │                │             │
│   ┌─────────────▼─────┐  ┌────────▼────────┐  ┌────▼─────────┐   │
│   │ LLM Provider 抽象 │  │ Tool Registry   │  │ Event Bus    │   │
│   │ · codex_oauth     │  │ · 行情 / 资讯   │  │ · session 流 │   │
│   │ · anthropic_*     │  │ · 技术指标       │  │ · 推到所有   │   │
│   │ · openai_*        │  │ · corpus 检索   │  │   订阅 transport│
│   │ (HTTP 直连，无SDK)│  │ · vault 读写    │  │              │   │
│   └─────────┬─────────┘  └────────┬────────┘  └──────────────┘   │
└─────────────┼─────────────────────┼─────────────────────────────-┘
              │ 远程 HTTP            │
              ▼                     ▼
        ┌──────────┐        ┌──────────────┐    ┌──────────────────┐
        │ LLM      │        │ Corpus       │    │ Vault            │
        │ Backends │        │ (静态资源)   │    │ (SQLite, 单库    │
        │          │        │              │    │  多 user_id)     │
        │ codex /  │        │ markdown +   │    │                  │
        │ Anthropic│        │ wikilink     │    │ sessions /       │
        │ / OpenAI │        │              │    │ decisions /      │
        │          │        │ ~/playground/│    │ holdings /       │
        │          │        │ finance-     │    │ reviews /        │
        │          │        │ giant/corpus/│    │ mandates / ...   │
        └──────────┘        └──────────────┘    └──────────────────┘
```

## 3. 核心组件

### 3.1 Gateway（Rust 长跑 daemon）

唯一持有 lifecycle 的进程。负责：
- HTTP / SSE / WebSocket 三种 transport（axum + tokio）
- 多 adapter 的请求路由
- Session 管理 + user 作用域
- LLM provider 调度（重试 / 降级 / quota 追踪）
- Tool 注册与调用分发
- Event Bus 中央事件流

部署单元：**单二进制**（`leek-gateway`），静态链接 SQLite，启动即用。详见 [ADR-0001](decisions/0001-rust-gateway.md)。

### 3.2 Agent Core（自实现 harness）

跑在 gateway 进程内，不是独立服务。每个 session 一个 loop instance。核心循环：

```
user input
  ↓
[ Agent Loop ]
  · 拼装 system prompt（含 corpus 上下文 + vault 上下文 + mandate）
  · LLM 调用（流式接收）
  · 解析 tool calls
  · 并行执行 tools（每个 tool 一个 task）
  · tool 结果回填上下文
  · 决定继续思考还是返回最终回复
  ↓
最终回复 / 中间事件流 → Event Bus → 订阅的 transport → adapter
```

详见 [ADR-0005](decisions/0005-self-implemented-harness.md) 和后续的 `p1-spec/agent-loop.md`。

### 3.3 LLM Provider 抽象

```rust
trait LlmProvider {
    auth: ApiKey | OAuth(provider_kind)
    capabilities: { thinking, tool_use, vision, ... }
    chat(messages, tools, opts) -> Stream<LlmEvent>
}
```

P1 支持的 provider：
- `codex_oauth` —— 复用用户 ChatGPT 订阅 quota（开发期默认）
- `anthropic_api_key`
- `openai_api_key`

后续：`anthropic_oauth`、`deepseek_api_key`、`ollama_local`。

**关键约束**：所有 provider 走 HTTP 直连，**不依赖第三方 SDK**。新 feature 通过改 JSON 字段就能跟上，不被 SDK 升级节奏拖累。详见 [ADR-0005](decisions/0005-self-implemented-harness.md)。

### 3.4 Tool Registry

P1 工具清单：
- **行情**：实时报价、K 线、成交、财务三表
- **资讯**：新闻、公告、研报抓取
- **技术指标**：MA / MACD / RSI / 布林等 ~20 个常用指标
- **Corpus 检索**：按 wikilink 解析、全文检索、概念图谱遍历
- **Vault 读写**：sessions / decisions / holdings / reviews / mandates 的 CRUD

**不进 P1**：组合优化（mean-variance / Black-Litterman）、严肃回测、衍生品定价。这些是 P3+，要时再单开 Python sidecar。

详见 `p1-spec/tools.md`（待写）。

### 3.5 Event Bus

Gateway 内部一份发布订阅总线，**所有事件都长一个样子**：

```
Event {
    session_id: Uuid,
    user_id: String,
    kind: EventKind,           // user_message | agent_thinking | tool_call_start |
                               // tool_call_result | panel_update | reasoning_dag_node | ...
    payload: Json,
    ts: ISO8601
}
```

订阅方包括所有活跃 transport（SSE 流、WebSocket 连接）和内部组件（持久化、metrics）。详见 [ADR-0007](decisions/0007-event-protocol-and-transports.md)。

## 4. Adapters

| Adapter | P1 / P2+ | Transport | 角色 |
|--|--|--|--|
| **Web (chat-canvas)** | P1 | HTTP POST + SSE | 人主用入口；SolidJS 实现 |
| **MCP HTTP** | P1 | streamable-http (HTTP POST + SSE) | 让其他 agent 把 leek 的工具当 MCP 工具调用 |
| **CLI** | P2 | HTTP POST + WebSocket | shell 一句话查询 / 长任务订阅 |
| **TUI** | P2 | HTTP POST + WebSocket | 离线终端版的 chat-canvas |
| **Claude Code skill 包** | P2 | HTTP POST + WebSocket（skill 内 fetch） | 在 Claude Code 里把 leek 当一等公民 |
| **ACP** | 砍 | — | 详见 [ADR-0004](decisions/0004-no-acp.md) |
| **Messaging (Slack / IM)** | P3+ | webhook | 移动场景 |

**关键设计**：所有 adapter 都是无 lifecycle 的"门"——关 Web tab / 关 Claude Code 不影响 gateway 本体。Gateway 死了所有 adapter 才不可用。

## 5. 数据层

### 5.1 Corpus（静态资源）

- **物理位置**：作为 leek 仓库的 git submodule 挂在 `./corpus/`（GitHub 上指向 `hchen13/the-corpus`）。同一个 repo 也作为 sibling 仓库 `~/playground/finance-giant/` 的 submodule 存在，用于独立的 corpus 维护工作流——两个 working copy 是同一份内容
- **配置覆盖**：`corpus_path` 配置项默认是 leek 仓库内的 `./corpus/`，可覆盖指向其他 working copy（如 `~/playground/finance-giant/corpus/`）以与 corpus 维护工作流共用
- **内容**：universal、可公开、对任何用户都成立的投资智慧（Buffett / Munger / Dalio 思想 + 概念页 + entity 档案）
- **形态**：markdown + frontmatter + wikilink，对 leek 而言**像项目的静态资源**——读多、几乎不写、由独立流程维护
- **agent 写入边界**：agent 永远不直接编辑 `wikis/` 或 `sources/`；如果 agent 在某 session 里产出了"值得进 corpus"的候选，最多只能写到 `corpus/inbox/`（未来 promotion pipeline 启用时）。**P1 不必做 promotion pipeline。**

详见 [ADR-0003](decisions/0003-corpus-as-static-resource.md)。

### 5.2 Vault（SQLite，单库多 user_id）

- **位置（本地）**：`~/.leek/vault.db`
- **位置（cloud）**：单库横扩到 PostgreSQL（schema 不变，driver 切换）
- **内容**：per-user runtime state——sessions、messages、decisions、holdings（portfolio）、reviews、mandates、watchlists、artifacts
- **隔离方式**：每张表第一列都是 `user_id`，所有查询都带 user_id 过滤；本地默认 `user_id = "local"`

详见 [ADR-0002](decisions/0002-sqlite-vault-single-db.md) 和后续的 `p1-spec/data-schema.md`。

### 5.3 Hybrid Storage 与跨域引用

系统刻意是 **hybrid storage**：
- Corpus = 文件 + git
- Vault = SQLite

跨域引用走**软引用**：vault 里某行 decision 的 `corpus_refs` 字段存 corpus 路径字符串数组（如 `["wikis/principles/margin-of-safety.md"]`），渲染时 resolver 拿字符串去文件系统读。**不做双向 wikilink resolver**。

## 6. 通信协议

### 6.1 事件协议

所有客户端（Web / 外部 agent）通过统一事件协议接收 gateway 推送：

```jsonc
{
  "session_id": "...",
  "user_id": "local",
  "kind": "agent_thinking" | "tool_call_start" | "tool_call_result"
        | "panel_open" | "panel_update" | "reasoning_dag_node"
        | "agent_message" | "agent_done" | "error",
  "payload": { /* kind-specific */ },
  "ts": "2026-05-01T14:30:00.123Z"
}
```

具体 `kind` 列表 + payload schema 在 `p1-spec/api.md`（待写）里展开。

### 6.2 Transport 选择

| 用户场景 | 提交 | 接收 | Transport |
|--|--|--|--|
| 浏览器 chat | HTTP POST `/sessions/:id/messages` | GET `/sessions/:id/events` (SSE) | SSE |
| 外部 agent 长任务 | HTTP POST `/sessions` | WebSocket `/sessions/:id/ws` | WebSocket |
| MCP HTTP | streamable-http | 同上 | MCP 协议自带 |

浏览器侧后续如果要支持"用户中途打断"或"协作"，无缝升级到 WebSocket，**事件协议不变**。详见 [ADR-0007](decisions/0007-event-protocol-and-transports.md)。

## 7. Agent Loop 概览

```
                 ┌───────────────────────────────────────────────┐
                 │ Session State (in memory + persisted to vault)│
                 │  · messages[]                                 │
                 │  · scratchpad                                 │
                 │  · open panels[]                              │
                 │  · reasoning DAG (current task)               │
                 └───────────────┬───────────────────────────────┘
                                 │
        user_message             │
            │                    ▼
            ▼            ┌──────────────────┐
   ┌──────────────────┐  │ Build Context    │
   │ Loop Iteration   │←─│  · system prompt │
   │  ┌────────────┐  │  │  · corpus refs   │
   │  │ LLM Stream │  │  │  · vault state   │
   │  └─────┬──────┘  │  │  · mandate       │
   │        │ events  │  └──────────────────┘
   │        ▼         │
   │ ┌────────────┐   │
   │ │ Detect Tool│   │
   │ │ Calls      │   │
   │ └─────┬──────┘   │  no calls → reply finalized → done
   │       │ has calls│
   │       ▼          │
   │ ┌────────────┐   │
   │ │Execute     │   │ tool 结果回填 messages[]
   │ │Tools 并行  │───┼──────────────────────┐
   │ └─────┬──────┘   │                      │
   │       │          │                      │
   └───────┴──────────┘                      │
           ▲                                 │
           └─────────────────────────────────┘
```

每个内部 phase 都向 Event Bus 推一条事件 → 订阅的 transport → 前端 panel 更新。

详见 `p1-spec/agent-loop.md`（待写）。

## 8. Multi-user from Day 1

即使 P1 只有一个本地用户，physical multi-user 痕迹必须从 day 1 存在：

1. Gateway 启动接受 `user_id`（默认 OS 用户名 / 配置文件）
2. 每个 vault 表的 `user_id` 列必须存在，所有查询带过滤
3. Corpus 永远 read-mostly，agent 写入路径受协议约束（只能写 inbox/）
4. Session 元数据带 user_id，Event Bus 推送时以 user 为单位隔离

这三条不增加 P1 复杂度，但 cloud 切换时不必返工。

## 9. 部署形态

### 9.1 本地（默认 / 开发 / 个人使用）

- 单二进制 `leek-gateway` 长跑（`launchctl` / `systemd` / 手动 `leek serve`）
- Web 前端可以打到 binary 内 embed 静态资产，`/` 路径直接返回 SPA
- SQLite 文件 `~/.leek/vault.db`
- Corpus 默认路径 = leek 仓库内的 submodule `./corpus/`（启动时由 `corpus_path` 配置项指向，未配置时按二进制查找 `./corpus/`，可覆盖指向 sibling 仓库的 working copy）
- LLM 走外部 HTTP（codex / Anthropic / OpenAI）

启动后只需要一个 URL（如 `http://localhost:8964`）就能用。

### 9.2 Cloud（未来）

- 同二进制 + 单 Postgres（schema 不变）
- 多 user_id 隔离已在 day 1 做好
- Corpus 由官方维护，per-user vault 横扩

## 10. P1 范围

### P1 做什么

- Gateway 长跑（Rust）
- Agent Core 自实现 harness（loop + scratchpad + thinking + tool use）
- LLM provider：codex_oauth + anthropic_api_key + openai_api_key
- Tool 集：行情 / 资讯 / 技术指标 / corpus 检索 / vault 读写
- Adapter：Web (chat-canvas SolidJS) + MCP HTTP
- Vault：SQLite 单库多 user_id
- Corpus 只读消费

### P1 明确不做

- ❌ Paper trading（[ADR-0008](decisions/0008-no-paper-trading.md)）
- ❌ 组合优化 / 严肃回测 / 衍生品定价
- ❌ ACP adapter（[ADR-0004](decisions/0004-no-acp.md)）
- ❌ 自动 promotion pipeline 写 corpus（[ADR-0003](decisions/0003-corpus-as-static-resource.md)）
- ❌ Multi-account / 杠杆 / 保证金管理
- ❌ TUI / CLI / Claude Code skill / 桌面 dock app（P2+）
- ❌ Daily briefing 主动推送（推迟）

### P1 非目标但要为之留位

- Promotion pipeline 写 inbox（接口预留，P1 不实现）
- ACP / 桌面 dock / Messaging（架构允许扩展，P1 不做）
- 真实下单（PortfolioOps trait 接口预留，driver 不实现）

## 11. 决策索引（ADR）

| # | 决策 | 链接 |
|--|--|--|
| 0001 | Gateway 用 Rust | [→](decisions/0001-rust-gateway.md) |
| 0002 | Vault = SQLite，单库多 user_id | [→](decisions/0002-sqlite-vault-single-db.md) |
| 0003 | Corpus 视为静态资源 | [→](decisions/0003-corpus-as-static-resource.md) |
| 0004 | P1 不做 ACP adapter | [→](decisions/0004-no-acp.md) |
| 0005 | 自实现 harness + LLM provider 抽象 | [→](decisions/0005-self-implemented-harness.md) |
| 0006 | 前端 = SolidJS | [→](decisions/0006-frontend-solidjs.md) |
| 0007 | 事件协议 + SSE/WebSocket 双 transport | [→](decisions/0007-event-protocol-and-transports.md) |
| 0008 | P1 不做 paper trading | [→](decisions/0008-no-paper-trading.md) |
| 0009 | Portfolio = 投研参考视图 | [→](decisions/0009-portfolio-as-research-context.md) |

## 12. 后续 spec 文档（依赖此架构）

| 文档 | 状态 | 阻塞什么 |
|--|--|--|
| `frontend/concept.md` | 第 1 波 | UX 设计起点 |
| `frontend/panels.md` | 第 1 波 | UX 设计 panel 类型清单 |
| `p1-spec/api.md` | 第 2 波 | 前端连真数据、外部 agent 接入 |
| `p1-spec/agent-loop.md` | 第 2 波 | harness 实现 + 前端事件渲染 |
| `p1-spec/tools.md` | 第 2 波 | tool 实现排期 |
| `p1-spec/data-schema.md` | 第 2 波 | vault 实现 + migration |
| `p1-spec/llm-provider.md` | 第 2 波 | provider 实现 + Codex OAuth 流程 |
| `roadmap.md` | 第 2 波 | 实施排期 |

## 13. 历史与演化

- 项目脱胎自 `finance-giant/` 仓库的 corpus 工作。2026-04-30 之前，agent 系统只是 `finance-giant/agent-project/` 子目录里的设计草稿。
- 2026-05-01 正式拆为独立仓库 `~/playground/leek/`，CLI 名 `leek`。
- 设计讨论的 anchor 文档 `design/handoff-2026-05-01.md` 保留作历史，本文档（`architecture.md`）取代它成为权威。
