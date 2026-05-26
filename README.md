# L.E.E.K (老韭菜)

**Logic-Enhanced Equity Kernel** is an early-alpha investment research operating system. It combines a long-running agent gateway, a chat-canvas web workbench, market and filing tools, and a curated investing corpus so research sessions can move from question to evidence to decision discipline.

![L.E.E.K web workbench](docs/assets/leek-workbench.png)

## English

L.E.E.K is built for investors who want a research workflow that is more auditable than a plain chatbot and more decision-oriented than a static notebook. A session keeps the conversation on the left, expands tool evidence and artifacts into the center canvas, and keeps corpus context plus the agent plan visible on the right.

The project is currently an early alpha. It is useful as a local research workbench and harness-engineering playground, but it is not production investment infrastructure and it does not provide financial advice.

### What It Does

- Runs a local gateway daemon behind multiple possible adapters.
- Presents a web workbench with chat, canvas artifacts, market panels, plan state, and corpus context.
- Grounds research with tool calls such as market snapshots, financial statements, filings, web search, and corpus retrieval.
- Separates the universal corpus from user-specific vault state such as sessions, decisions, holdings, mandates, and reviews.
- Keeps A-share research workflows first-class while preserving the broader multi-market architecture.

### Architecture

- **Gateway**: Rust service and CLI binary named `leek`.
- **Web workbench**: SolidJS + Vite frontend under `frontend/web`.
- **Vault**: local runtime state, usually a SQLite database selected with `--vault`.
- **Corpus**: curated investing knowledge under `corpus/`, treated as read-mostly knowledge.
- **Promotion path**: durable knowledge should move through human review before becoming part of the corpus.

### Local Development

Build the gateway:

```bash
cargo build -p leek-gateway
```

Start the gateway with a local development vault:

```bash
target/debug/leek --vault tmp/dev/vault.db serve --port 8964
```

In another terminal, start the web workbench:

```bash
npm --prefix frontend/web install
npm --prefix frontend/web run dev -- --host 127.0.0.1 --port 5173
```

Then open:

```text
http://127.0.0.1:5173
```

### Development Checks

```bash
cargo test -p leek-gateway
npm --prefix frontend/web run build
```

## 中文

L.E.E.K（老韭菜）是一个早期 alpha 阶段的投研操作系统。它的目标不是做一个普通聊天机器人，而是把投研问题、工具证据、画布产物、语料库上下文和最终决策纪律放在同一条可审计链路里。

当前界面采用 chat-canvas 形态：左侧是对话主轴，中间是工具证据、行情、财务和图表画布，右侧是 corpus brain 与 agent plan。这样可以同时看到“它答了什么”“依据是什么”“当前计划走到哪里”。

项目目前仍在快速演进中。它可以作为本地投研工作台和 harness engineering 实验场，但不是生产级投资基础设施，也不构成任何投资建议。

### 能做什么

- 在本地运行一个长期存活的 gateway daemon。
- 提供带聊天、画布、市场面板、计划状态和 corpus 上下文的 Web 工作台。
- 通过行情快照、财务报表、公告材料、网页检索和 corpus retrieval 等工具补齐研究证据。
- 区分通用 corpus 与用户自己的 vault 状态，例如 session、decision、holding、mandate 和 review。
- 把 A 股研究工作流作为一等场景，同时保留未来扩展到多市场的架构边界。

### 架构

- **Gateway**：Rust 服务与 CLI，二进制名为 `leek`。
- **Web 工作台**：位于 `frontend/web` 的 SolidJS + Vite 前端。
- **Vault**：本地运行时状态，通常通过 `--vault` 指定 SQLite 数据库。
- **Corpus**：位于 `corpus/` 的 curated investing knowledge，默认按 read-mostly 使用。
- **Promotion path**：值得沉淀的知识先经过人工 review，再进入正式 corpus。

### 本地开发

构建 gateway：

```bash
cargo build -p leek-gateway
```

用本地开发 vault 启动 gateway：

```bash
target/debug/leek --vault tmp/dev/vault.db serve --port 8964
```

另开一个终端启动 Web 工作台：

```bash
npm --prefix frontend/web install
npm --prefix frontend/web run dev -- --host 127.0.0.1 --port 5173
```

然后打开：

```text
http://127.0.0.1:5173
```

### 开发检查

```bash
cargo test -p leek-gateway
npm --prefix frontend/web run build
```
