# L.E.E.K P1 Roadmap

> P1 实施排期：按依赖顺序的里程碑、各 milestone 的验收标准、并行 vs 串行的判断、风险点。

P1 总目标：**用户能在 leek 里完成完整的"创建任务 → agent 执行 → review deliverable → confirm 决策 → 持久化"闭环**，并使用 Web (chat-canvas) + MCP HTTP 两种接入方式。

预估总周期：**12-16 周**（不含 UX 设计与素材制作的并行时间）。

---

## 总览

```
            UX 设计（claude design 出稿 → 你审 → 迭代）
            ───────────────────────────────────────→ 与开发并行（不阻塞 M0-M2）
                                                    需要 UX 稿才能动手 M5-M6

开发：
  M0 ─ M1 ─ M2 ─ M3 ─ M4 ─ M5 ─ M6 ─ M7
  │    │    │    │    │    │    │    │
  │    │    │    │    │    │    │    └─ 验收 / polish
  │    │    │    │    │    │    └────── 决策闭环 / Portfolio
  │    │    │    │    │    └─────────── Frontend 核心 panels
  │    │    │    │    └──────────────── Task lifecycle + subagent
  │    │    │    └───────────────────── 工具集 (basic 6 类)
  │    │    └────────────────────────── LLM provider + Codex OAuth
  │    └─────────────────────────────── Gateway + Harness skeleton
  └──────────────────────────────────── 项目骨架
```

---

## M0 — 项目骨架（Week 1）

**目标**：建立可工作的代码仓库框架，所有人能在同一基础上协作。

### 任务

- [ ] Cargo workspace 初始化（`leek-gateway` binary + 各 crate 模块）
  - `crates/leek-gateway` (binary)
  - `crates/leek-vault` (SQLite + sqlx)
  - `crates/leek-llm` (provider abstraction)
  - `crates/leek-tools` (tool registry)
  - `crates/leek-agent` (harness)
  - `crates/leek-corpus` (corpus loader)
  - `crates/leek-event-bus`
  - `crates/leek-types` (共享类型)
- [ ] `pnpm` workspace 初始化（前端）
  - `frontend/web` (SolidJS app)
  - `frontend/types` (TypeScript 共享类型，从 Rust 自动生成)
- [ ] CI pipeline（GitHub Actions）：
  - Rust: `cargo fmt --check` / `cargo clippy` / `cargo test` / `cargo build --release`
  - Frontend: `pnpm typecheck` / `pnpm lint` / `pnpm build`
- [ ] Pre-commit hook：rustfmt + clippy + eslint
- [ ] Logging 配置（`tracing` + 默认 stdout JSON 格式）
- [ ] 配置加载（`figment` + `~/.leek/config.toml`）
- [ ] `vault.db` 路径解析 + `corpus_path` 解析 + 错误处理

### 验收

- `cargo build --release` 出 binary
- `leek-gateway --help` 显示子命令（`serve` / `init` / `migrate`）
- `leek-gateway init` 在 `~/.leek/` 创建默认配置
- `leek-gateway serve` 启动后报错优雅（无 provider 配置时）
- 前端 `pnpm dev` 启动空 SolidJS app

### 风险

- Cargo workspace 依赖管理 / sqlx 编译期 SQL 校验初期会卡（需要离线模式或 CI 上 vault.db 模板）→ 提前准备 `sqlx prepare` workflow

---

## M1 — Gateway 骨架 + Vault（Week 2-3）

**目标**：HTTP server 起来；vault SQLite 完整可写。

### 任务

- [ ] axum HTTP server skeleton（healthcheck endpoint）
- [ ] tracing middleware + auth middleware（P1 = none mode）
- [ ] Vault：sqlx + 完整 schema migration（`0001_initial.sql` 含 `data-schema.md` 定义的所有表）
- [ ] Vault repository 层：CRUD for users / sessions / tasks / messages / events / artifacts
- [ ] EventBus：tokio broadcast channel + per-session 订阅 + 持久化 consumer
- [ ] SSE endpoint 骨架（订阅 EventBus → 推 SSE）
- [ ] 单元测试：vault repos / event bus
- [ ] 集成测试：HTTP healthcheck + vault migration apply

### 验收

- `leek-gateway serve` 启动 < 500ms
- `curl http://localhost:8964/api/v1/health` 返回 200 + uptime
- vault.db 自动初始化 + migration apply
- 创建一个 mock event 写入 vault.events，订阅 SSE 能收到

### 并行

- 前端 `pnpm dev` 上 SolidJS skeleton + 路由（不连后端）

---

## M2 — LLM Provider + Codex OAuth（Week 3-4）

**目标**：三个 provider 都能从 leek 调通 LLM API；Codex OAuth device flow 跑通。

### 任务

- [ ] `LlmProvider` trait + 类型定义（`llm-provider.md` §2）
- [ ] `AnthropicProvider` 实现（含 SSE 解析）
- [ ] `OpenAiApiKeyProvider` 实现（Responses API）
- [ ] `CodexOAuthProvider` 实现：
  - Device flow（手动 curl 验证后写 Rust）
  - Token refresh
  - 与 OpenAI Responses API 同 endpoint 但 OAuth bearer
  - 必要 headers / UA / beta 标志（实测决定）
- [ ] `LlmRegistry` + fallback chain
- [ ] `llm_usage_log` 写入
- [ ] HTTP API：`/api/v1/providers/*`
- [ ] Settings 页面 backend 部分（提供 Provider 配置 UI 数据）
- [ ] 集成测试：用 mocked SSE server 验证 stream 解析

### 验收

- 一次完整 LLM 调用：从 HTTP request → SSE stream → 累积输出 → token 统计写 vault
- Codex OAuth device flow 在 CLI 跑通（人工 + 浏览器配合）
- API key path 跑通（Anthropic / OpenAI 都能调）
- Fallback chain 测试：mock 一个 always-fail provider，自动切到下一个
- 失败重试 / 超时处理 OK

### 风险

- **Codex OAuth 是最大不确定性**：
  - OpenAI 半官方接口，文档不稳定，可能临时调整 headers / 行为
  - 缓解：先 curl 验证 → 再写 Rust；保留 API key 路径作为兜底
  - 如果 OAuth 完全跑不通（OpenAI 政策限制），P1 就只支持 API key，OAuth 留 P2

---

## M3 — 工具集 (Basic 6 类)（Week 4-6）

**目标**：行情 / 资讯 / 财务 / 技术指标 / corpus / vault 这 6 类工具完整可用。

### 任务

- [ ] `Tool` trait + `ToolRegistry` + `ToolContext`
- [ ] **行情类**：`quote.get` / `quote.batch` / `chart.ohlc` / `chart.intraday`
  - 数据源：yahoo_finance_api（美股 / 港股）
  - 数据源：sina / netease 抓取（A 股，简化版）
- [ ] **资讯类**：`news.search` / `news.fetch`
  - 数据源：newsapi.org
- [ ] **财务类**：`financials.snapshot` / `financials.history`
  - 数据源：yahoo_finance_api
- [ ] **技术指标**：`indicator.compute`（MA / EMA / MACD / RSI / BOLL 共 5 个）
  - 库：ta-rs
- [ ] **Corpus 类**：
  - 启动期 corpus 扫描 → 内存图缓存
  - `corpus.search`（基于 tantivy 全文检索）
  - `corpus.read`
  - `corpus.graph`
- [ ] **Vault read 类**：所有 read 工具（holdings / decisions / reviews / watchlists / charter）
- [ ] 缓存层：moka 集成
- [ ] 单元测试每个 tool 至少 3 个 case
- [ ] 集成测试：corpus / vault tools 端到端

### 验收

- 每个工具调用 → 返回结果 → 写 `tool_call_runs`
- 缓存命中率统计可见
- 工具失败 graceful（结构化错误，不 panic）

### 并行

- M3 期间前端可以做：CorpusBrain 的图渲染 + 静态 graph 数据测试（用 corpus.graph 工具的 mock 数据）

---

## M4 — Task Lifecycle + Subagent + Vault Write Tools（Week 6-9）

**目标**：完整的 Agent Loop + Task lifecycle + Subagent 调度。

### 任务

- [ ] **Vault write tools**：`decision.draft` / `review.draft` / `holdings.update` / `panel.*` / `reasoning.*` / `clarify.ask_user`
- [ ] **Agent Loop**（`agent-loop.md`）：
  - `AgentLoop` struct + state
  - 主循环（control inbox / outcome 解析）
  - `build_chat_request` + system prompt template
  - `consume_stream` 流式解析
  - `dispatch_tools` 并行调用
  - Reasoning DAG 自动节点生成
- [ ] **Subagent 调度**：
  - `subagent.spawn` tool
  - SubagentRunner（独立 LLM loop + 工具子集）
  - 5 个 subagent specs（valuation_dcf / news_summary / ticker_research / comparison_pair / free_form）
  - `vault.subagent_runs` 持久化
- [ ] **Task Lifecycle**：
  - HTTP API: `/api/v1/tasks/*`
  - Task scheduler（pick from queued → start AgentLoop）
  - Control message 端点（`/tasks/:id/control`）
  - Task state machine（draft / queued / in_progress / awaiting_user / delivered / confirmed / rejected / cancelled / failed）
- [ ] **Deliverables**：
  - HTTP API: `/api/v1/deliverables/*`
  - confirm/reject 派生写入（decision / review）
- [ ] **Team Charter**：
  - HTTP API: `/api/v1/charter`
  - 注入 system prompt 的逻辑
- [ ] 集成测试：
  - 创建 task → main agent loop → 调多个 tool → 出 decision_draft → confirm → 写 vault.decisions
  - 创建 task 调 subagent → 主 agent 收 subagent 结果 → 继续推理 → 出 deliverable
  - 中途 control（追加约束 / 中断）正确生效

### 验收

- 从 HTTP `POST /tasks` 创建 task → 全流程跑通到 `confirmed` < 30s（含真实 LLM 调用）
- Subagent 完整跑：spawn → 子循环 → return → merge
- 中断 / 追加约束在 yield point 内（最多 3s）生效

### 风险

- Agent Loop 是 P1 的最复杂模块，bug 多
  - 缓解：写完整集成测试 + e2e 测试，覆盖 5+ 完整 task 场景
- LLM 协议差异：Anthropic / OpenAI 的 thinking / tool use 协议差异
  - 缓解：每个 provider 写独立 SSE 转换器 + snapshot 测试

---

## M5 — Frontend 核心 Panels（Week 8-12，与 M4 部分并行）

**前置**：UX 设计稿已经从 claude design 拿到（concept + panels 已经给它，并行迭代）。

**目标**：Web 前端的核心 panel 都能渲染，连真实 SSE 流。

### 任务（按 panels.md §20 排期分波）

#### 第一波（M8-M9）：产品 signature
- [ ] **CorpusBrain**：PixiJS WebGL 实现 + 力导布局 + 节点激活动效（3 强度）
- [ ] **ReasoningDAG**：DOM + SVG 实现 + 流式节点入场 + traveling pulse + subagent 子分支
- [ ] CorpusBrain ⟷ ReasoningDAG 联动

#### 第二波（M9-M10）：任务核心 UX
- [ ] **TaskBoard** 首页
- [ ] **TaskCreator**（结构化卡片）
- [ ] **TaskCard**（状态视觉差异 + 干预按钮 + thread）

#### 第三波（M10-M11）：核心闭环
- [ ] **DecisionDraft** panel（form + mandate_check 实时计算 + Confirm/Reject 仪式动作）
- [ ] **Review** panel
- [ ] **Portfolio** panel（含 CSV import + 历史快照切换）

#### 第四波（M11）：数据 panel
- [ ] **Quote**（高频字段 ref 更新）
- [ ] **Chart**（lightweight-charts 集成）
- [ ] **Article**（markdown 渲染）
- [ ] **Table**（TanStack Table）
- [ ] **WatchList**

#### 第五波（M11-M12）：agent 状态 + 设置
- [ ] **Reasoning**（thinking traces 折叠）
- [ ] **Plan**（任务执行计划）
- [ ] **ToolCall**（实时进度）
- [ ] **ProviderConfig**（API key + Codex OAuth 引导）
- [ ] **CharterEditor**（Team Charter 可视化编辑）

#### 基础设施
- [ ] SolidJS app skeleton + 路由
- [ ] SSE client（含 Last-Event-ID 续传）
- [ ] EventBus → Solid signals 桥接
- [ ] Panel container（拖动 / 缩放 / 钉住 / 关闭 + persist layout）
- [ ] Mention chip 系统
- [ ] 暗色主题 + 字体 + 视觉系统（按 UX 视觉稿）

### 验收

- 用户能在 web UI 完整跑一个 task（创建 → 看 agent 工作 → 看 deliverable → confirm）
- CorpusBrain 激活动效流畅（60fps，100+ 节点）
- ReasoningDAG 流式展开顺滑
- 高频 quote 数字 100Hz 跳动 CPU < 30%

---

## M6 — 决策闭环 + Portfolio 数据流（Week 12-14）

**目标**：完整的端到端决策与持仓追踪闭环。

### 任务

- [ ] Portfolio CSV import 解析（5 种主流券商格式）
- [ ] Portfolio 编辑（添加 / 删除 / 行内 edit）
- [ ] Decision review schedule cron（启动期扫描 → 创建 review task）
- [ ] Mandate check 实时计算（前后端双实现，前端用于 form 即时反馈，后端用于权威结果）
- [ ] Watchlists CRUD UI
- [ ] MCP HTTP server 实现（暴露 read-only 工具子集）
- [ ] WebSocket endpoint 实现（外部 agent 双向通道）

### 验收

- 完整 reactive task：创建 → 决策 → confirm → portfolio 更新提示
- 完整 proactive task：cron 触发 review → 用户 review confirm
- MCP HTTP client 能连上 leek 调用 corpus.search 等工具
- WebSocket：外部模拟 agent 能完整跑一个 task

---

## M7 — Polish + E2E + 文档（Week 14-16）

**目标**：发布前的体验打磨与稳定性。

### 任务

- [ ] E2E 测试场景覆盖（≥ 10 个完整 task 场景）
- [ ] 性能 profiling：长跑 24h 不崩 / 内存 < 200MB / 延迟 p95 达标
- [ ] 错误处理路径完整（provider 全 fail / 网络断开 / vault 锁等）
- [ ] Onboarding 流程（首次启动引导 + Provider 配置 + Charter 模板）
- [ ] CSV import 健壮性（异常格式不 crash）
- [ ] 数据备份命令 `leek vault backup`
- [ ] 用户文档（如何启动 / 如何配置 provider / 如何更新 corpus）
- [ ] Logging / 错误信息打磨（用户友好）
- [ ] CHANGELOG + 版本号 v0.1.0

### 验收

- E2E 测试全绿
- 长跑 24h 内存稳定
- 完整新用户 onboarding < 5 分钟到第一个 task delivered

---

## 关键并行依赖

```
M0 → M1 → M2 → M3 ────────→ M4 ────────→ M6 → M7
                  │                       │
                  └─→ Frontend skeleton ─→ M5
                  
UX 设计 (claude design) ────────────────→ 提供给 M5

P1-spec 细化 ──→ 持续修订各 spec 文档（不阻塞实施）
```

并行机会：

- **M0 完成后**前端可以独立先做骨架（路由 / 主题 / 静态页面）
- **M3 期间**前端可以做 CorpusBrain（用 mock corpus.graph 数据）
- **M4 期间**前端可以做 TaskBoard / TaskCreator（用 mock 任务数据）
- **M5 期间**后端做 M6 的 portfolio + MCP HTTP

---

## P1 范围外（明确不做）

| 类别 | 不做 | 原因 / 推迟到 |
|--|--|--|
| Adapter | TUI | P2 |
| Adapter | CLI 一等公民（仅 `leek serve` / `leek init` 等管理命令） | P2 |
| Adapter | Claude Code skill 包 | P2 |
| Adapter | ACP | 不做（[ADR-0004](decisions/0004-no-acp.md)） |
| Adapter | Messaging / Slack | P3 |
| Adapter | 桌面 dock app | P3 |
| 功能 | Paper trading | 不做（[ADR-0008](decisions/0008-no-paper-trading.md)） |
| 功能 | 真实下单 | 永远不做（合规） |
| 功能 | 组合优化 / 严肃回测 / 衍生品定价 | P3 |
| 功能 | 自动 promotion pipeline 写 corpus | 不做（[ADR-0003](decisions/0003-corpus-as-static-resource.md)） |
| 功能 | Daily briefing 主动推送 | P2 |
| 功能 | 多 agent 协作（multi-persistent specialist） | P2 / 视 corpus 演进 |
| 功能 | 协作 / 多人共享 | P3 |
| 功能 | 移动 native app | P3 |
| 高级 | OAuth / API key 之外的认证（如 SAML / SSO） | P2 / cloud 时 |
| 高级 | Heatmap / CorrelationMatrix panel | P2 |

---

## 关键里程碑日期（基于 12 周乐观估算）

假设 2026-05-04 启动：

| Milestone | 完成日期 | 关键产出 |
|--|--|--|
| M0 | 2026-05-10 | Cargo workspace + CI |
| M1 | 2026-05-24 | Gateway HTTP server + Vault |
| M2 | 2026-06-07 | LLM Provider + Codex OAuth 跑通 |
| M3 | 2026-06-21 | 6 类工具完整可用 |
| M4 | 2026-07-12 | Task Lifecycle + Subagent + Vault write |
| M5 | 2026-08-02 | Frontend 17 类核心 panel |
| M6 | 2026-08-16 | 决策闭环 + Portfolio + MCP HTTP + WebSocket |
| M7 | 2026-08-30 | 发布 v0.1.0 |

**v0.1.0 发布目标**：2026-09 月初（4 个月）。

---

## 风险登记

| 风险 | 概率 | 影响 | 缓解 |
|--|--|--|--|
| Codex OAuth 跑不通 | 中 | 中 | API key 兜底；OAuth 推迟到 P2 |
| LLM 协议演化（thinking / tool use 改版） | 中 | 中 | 每个 provider 独立适配器；不 vendor SDK |
| Agent Loop 复杂度高于预估 | 高 | 高 | 完整集成 + e2e 测试；增加 M4 时长 |
| Frontend 性能（CorpusBrain WebGL）| 中 | 中 | 提前 prototype；最差降级到 Canvas 2D |
| UX 设计与 spec 错位 | 中 | 中 | 持续 iterating spec → UX → spec；M5 启动前对齐一次 |
| 数据源不稳定（yahoo_finance / newsapi 等） | 中 | 低 | 多源备份；缓存层兜底 |
| 用户实际使用反馈与设计差异大 | 中 | 高 | M7 提前邀请少量用户测试；保留快速迭代窗口 |

---

## P1 之后：P1.5 / P2 候选清单（暂存）

P1.5（短期补强，发布后 1-2 月内）：
- 用户中途打断的细粒度（流式中断而非 yield-point 中断）
- daily briefing
- daily review schedule 自动启动（用户授权后 auto-execute）
- LLM usage 详细分析页面
- Reasoning DAG 激活路径回放

P2（半年内）：
- TUI adapter
- Claude Code skill 包
- 桌面 dock app prototype
- 多 charter 切换（不同投资策略）
- Subagent 嵌套（非 P1 限制的 1 层）
- Heatmap / CorrelationMatrix panel
- 用户自定义 subagent spec
- corpus promotion pipeline（候选写 inbox）

P3+（一年外）：
- 真 multi-agent（视 corpus 演进决定）
- 真实券商 API 接入（视用户 / 合规）
- 组合优化 / 严肃回测
- 移动 native app
- 协作 / 多人共享 / cloud 部署
