# ADR 0001 — Gateway 用 Rust 实现

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0005](0005-self-implemented-harness.md)（harness 形态依赖语言选择）

## Context

Gateway 是 L.E.E.K 的唯一长跑进程，承载 transport 层（HTTP / SSE / WebSocket）、agent loop、tool registry、event bus、session 管理、SQLite vault 访问。它的性能与部署形态决定整个系统的体感。

候选语言三条路线：

| 语言 | 优势 | 劣势 |
|--|--|--|
| **Python** | 与 corpus 工具一致；金融数据科学库（pandas / yfinance / pyportfolioopt）丰富；hermes / DeerFlow 可直接抄 | 部署复杂（Docker / pyinstaller / uv）；并发性能弱；GIL；多 transport 长跑场景需要 asyncio + 慎重调优 |
| **TypeScript + Bun** | 前后端语言统一；分发快（Bun 单二进制）；dexter 路线 | 金融生态弱；Bun 生态仍年轻；事件总线需要自己实现 |
| **Rust** | 性能 / 内存 / 并发顶级；单二进制部署；axum + tokio + sqlx 成熟；编译期严格 | LLM SDK 一等公民弱；无 pyportfolioopt 类库；学习曲线 |

项目所有者明确目标：**高性能 + 极简部署**。

## Decision

**Gateway 用 Rust 实现，单二进制部署。**

技术栈基线：
- 异步运行时：`tokio`
- HTTP 框架：`axum`（含 SSE 与 WebSocket 一等公民支持）
- SQL：`sqlx`（编译期检查 SQL）+ `rusqlite`（如需嵌入）
- HTTP 客户端：`reqwest`
- 流式事件解析：`eventsource-stream` / 手写 SSE parser
- JSON：`serde` + `serde_json`
- 日志：`tracing` + `tracing-subscriber`
- 时间：`chrono` 或 `time`
- ID：`uuid`
- 配置：`figment` 或 `config-rs`

## Consequences

### LLM 调用走 HTTP 直连，不依赖 SDK

Anthropic / OpenAI / Codex 都暴露 REST + SSE 流式 API。Rust 用 `reqwest` + `serde_json` + 流式解析就完整搞定。**不引入任何 LLM SDK**——Anthropic 出新 feature 我们改 JSON 字段就行，不被 SDK 升级节奏拖累。

### 金融计算放工具层

复杂金融计算（DataFrame、技术指标、行情请求）作为 agent tool 暴露：
- DataFrame：`polars`（Rust-native，与 pandas 心智模型接近，pandas 都开始拿它当替代）
- 技术指标：`ta-rs`（替代 ta-lib）
- 行情：`yahoo_finance_api` / 直接调 Tushare / Wind / Alpha Vantage HTTP API
- 新闻 / 公告抓取：`reqwest` + `scraper` (HTML)

P1 工具范围**不含**组合优化（mean-variance / Black-Litterman）、严肃回测、衍生品定价。这些是 P3+ 议题；届时如需，单开 Python sidecar，**不破坏 P1 极简部署**。

### 部署形态：单二进制

- macOS / Linux 都用 `cargo build --release` 出 native binary
- 静态链接 SQLite（`bundled` feature）
- 前端 SPA build 后通过 `rust-embed` 打进二进制（可选）
- 启动：`leek serve --vault ~/.leek/vault.db --corpus ~/playground/finance-giant/corpus`

无 Docker、无 venv、无 npm install for runtime。

### 不可借鉴 hermes / DeerFlow / dexter 的代码

DeerFlow 是 Python，hermes 大概率也是 Python（待确认），dexter 是 Bun + TS。它们对我们只是**架构参考**，gateway pattern 的"长跑 daemon + 多 adapter 共用 EventBus"模式可以学，但**不能 vendor 代码**。

每个组件都要自己写一遍——这增加 P1 实施工作量，但避免 dependency hell。

### 项目所有者要心里有数的代价

- **LLM 新 feature 跟进**：Anthropic / OpenAI 出新形态（如新版 thinking、新 tool use 协议、新 reasoning 模式）时，Python / TS 通常先有官方 SDK 适配，Rust 端可能要自己写 HTTP 协议封装。可接受，但要预算时间。
- **量化分析复杂化时的边界**：P1 工具集设计为 Rust-native；如果未来 P2/P3 需要 mean-variance 优化、Monte Carlo、衍生品定价，要么自己实现（Rust 数学库够，但工作量大），要么开 Python sidecar（破坏单二进制承诺）。这个选择推迟到真要做时再做。
- **AI 编程辅助质量略弱**：Cursor / Copilot 对 Rust 的代码补全不如 Python / TS。开发节奏可能略慢。

## Alternatives Considered

### Python（被否）
- 与 corpus tools 一致 + 金融生态最强是真优势，但**部署复杂度直接破坏"极简部署"目标**。pyinstaller 打包 + 依赖体积 + asyncio 调优都是显性成本。
- LLM 推理走外部 HTTP 不需要 Python；金融计算可以作为工具子进程，不必整个 gateway 都 Python。

### TypeScript + Bun（被否）
- 前后端语言统一是 dexter 路线的甜点，但 Bun 还在 1.x 阶段，长跑 daemon 场景的稳定性未充分验证。
- 金融生态比 Python 还弱，DataFrame 类库（Danfo / Polars-JS）成熟度不及 Python / Rust。
- 性能不及 Rust，且 V8 内存占用对长跑 daemon 不友好。

### Hybrid: Rust gateway + Python sidecar（推迟）
- 现在不预设 sidecar——预设 sidecar 等于 P1 就放弃"极简部署"。
- 如果未来某 tool 必须用 Python（如 pyportfolioopt 没有等价物），到时单开 sidecar 即可，架构不阻挡。

### Go（被否）
- 编译产物简洁、并发好，但生态偏向云原生 / 后端服务，LLM / 数据科学库稀薄，比 Rust 没有显著优势，比 Python / Rust 都没有压倒性长项。

## 验证标准

P1 完成时应能验证：
- `leek-gateway` 单二进制 macOS 可运行，启动到接受第一个请求 < 500ms
- 长跑 24h 不崩，内存占用稳定（< 200MB 含 50 个 active session）
- LLM 流式响应延迟接近网络往返（gateway 自身开销 < 5ms / event）
- 单个 LLM call + 5 个 tool 并行的完整 turn < 2s（不含 LLM 推理时间）
