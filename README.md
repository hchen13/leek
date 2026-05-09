# L.E.E.K — Logic-Enhanced Equity Kernel

[English](#english) | [中文](#中文)

![L.E.E.K hero](docs/assets/leek-hero.png)

---

## English

L.E.E.K is an AI investment-research agent for retail investors. The core idea is **a corpus-grounded, plan-driven, transparent agent** — every claim is sourced, every reasoning step is visible on the canvas, and the agent's investing principles are persisted (and editable) in a corpus that evolves over time. The aim is to give a single user the kind of analytical scaffolding institutional desks have, without trying to replace their judgment.

### What it looks like

The web UI has four panels, each load-bearing:

| Panel | Role |
|---|---|
| **Chat** | The user's primary entry point — natural-language Q&A with the agent. Decision drafts, clarification requests, and final answers all land here. |
| **Canvas** | The agent's reasoning trace and tool-call audit. Every search, fetch, corpus read, subagent run, and intermediate thinking step renders as a card. The canvas is not decoration — in finance, "show your work" *is* the proof. |
| **Corpus** | The agent's investing brain — principles, mental models, named investors' frameworks. Persistent across sessions, edited like a wiki, and surfaced live as the agent retrieves from it. |
| **Plan** | The agent's working todo list. Visible to the user and used by the harness as a soft pacing signal — the user can see what's been done, what's pending, and where the agent is stuck. |

### Status (May 2026)

The repository has just entered the **`rebuild`** branch. The previous main-branch sprint shipped a workable but **over-engineered** harness (critic / plan-resolution lifecycle / 4-persona subagent / decision-draft pipeline / FMP financials). The architectural review at the end of that sprint concluded:

- Most of the deeper-research machinery served a goal (institutional-grade audit trails) that **is not the current priority**, and ended up distorting agent behavior on simple tasks.
- The right move is to **simplify back to a minimal, correct agent loop**, then build expertise on a single market (A-shares) before re-introducing rigor mechanisms one at a time.

The rebuild branch follows a milestone plan:

1. **Phase 0 — Slim down**: cut critic / decision_draft / record_investment_action / use_skill / delegate_research; replace plan_resolution lifecycle with simple checkbox-style markdown plans; static skill injection until M2.5.
2. **Milestone 1 — Agent-loop MVP**: turn-level cost cap, wall-clock timeout, tool-error consecutive limit, doom-loop detection, observability (`task_metrics`), plan-movement detector. (None of these have full reference implementations in Claude Code or codex-rs — leek is going stricter.)
3. **Milestone 2 — Corpus MVP**: live corpus panel, in-place corpus editing, corpus_read card polish.
4. **Milestone 2.5 — Skill / hook / plugin**: lifted directly from Claude Code (SKILL.md + YAML frontmatter, lazy load, hot reload; shell-command hooks with stdin/stdout JSON IPC; `.leek-plugin/plugin.json` plugin manifest).
5. **Milestone 3 — A-shares MVP**: 5–7 core tools (quote, candlesticks, capital flow, financials, filings, sector, macro), end-to-end runs of three task shapes (single-stock valuation, sector research, capital-flow read).
6. **Milestone 4 — A-shares completeness**: tool coverage, edge cases, time-boxed.

Multi-market expansion (US equities, crypto) and TUI come *after* the A-shares vertical is solid.

### Tech stack

- **Gateway** — Rust (`crates/gateway/`) — Axum HTTP, SQLx + SQLite, SSE event protocol.
- **Frontend** — SolidJS + Vite (`frontend/web/`) — chat panel, canvas, corpus brain widget, plan widget.
- **Vault** — single-file SQLite — sessions, messages, events, tasks, plans, decisions, corpus refs.
- **Corpus** — git submodule [`hchen13/the-corpus`](https://github.com/hchen13/the-corpus) — read-only investing wisdom; promoted via `corpus/inbox/` workflow.
- **LLM providers** — OpenAI Responses API (codex backend), with multi-provider abstraction in `crates/gateway/src/llm/`.

### Quick start

> Requires: Rust ≥ 1.80, Node ≥ 20, `pnpm`/`npm`, an OpenAI API key (codex OAuth) for the LLM, optional Tushare Pro key for A-shares financials.

```bash
# clone with corpus submodule
git clone --recursive https://github.com/hchen13/leek.git
cd leek

# build distilled corpus prompt (one-time per corpus version)
cargo run --bin leek -- corpus distill --root ./corpus

# start the gateway
cargo run --bin leek -- --vault ./vault.db serve --port 8964

# in another shell, start the frontend
cd frontend/web
npm install
npm run dev
# → http://localhost:5173
```

Per-user config goes in a `mandates/<user_id>.md` file — risk tolerance, position caps, instrument restrictions. The agent reads this every turn (no restart needed).

### Architecture pointers

- `AGENTS.md` — overall design discipline (read this first).
- `design/decisions/` — accepted-tradeoff ADRs (`0001..0012`).
- `design/p1-spec/` — module contracts (agent loop, API, corpus brain, data schema, tools).
- `harness/discipline.md` — the agent's behavioral rules (cited inline in system prompt).
- `harness/skills/<name>/SKILL.md` — task-type frameworks (currently `equity-valuation`, `crypto-research`).

### Contributing

The project is in active rebuild. Issues / PRs are welcome but expect a lot of churn until M3. The `main` branch is frozen at the end-of-sprint snapshot; all current work happens on `rebuild`.

---

## 中文

L.E.E.K（Logic-Enhanced Equity Kernel）是一个面向散户的 AI 投研代理。核心命题是 **基于 corpus、由 plan 驱动、过程透明的 agent** —— 每条结论必须有来源、每一步推理在 canvas 上可见、agent 的投研原则沉淀在一份会随时间迭代的 corpus 里。目标是把机构投研团队那套分析脚手架交到单个用户手里，而不是替用户判断。

### 界面

Web UI 有四个 panel，各自承担不可替代的角色：

| Panel | 作用 |
|---|---|
| **Chat（对话）** | 用户与 agent 的主入口 —— 自然语言提问。Decision draft、澄清问题卡、最终答复都在这里。 |
| **Canvas（画布）** | Agent 推理轨迹 + 工具调用的可审计视图。每次 search、fetch、corpus read、subagent 调用、中间思考都是一张卡。在金融分析里这不是装饰，"亮过程" 本身就是结论的凭证。 |
| **Corpus（知识库）** | Agent 的投资大脑 —— 投资原则、心智模型、知名投资人 framework。跨 session 持久化，可以像 wiki 一样编辑，agent 检索时实时高亮。 |
| **Plan（计划）** | Agent 当前在做的 todo list。用户可见，harness 用它作 soft pacing 信号 —— 看到 agent 完成了什么、还差什么、有没有卡住。 |

### 项目状态（2026 年 5 月）

仓库刚进入 **`rebuild`** 分支。上一阶段在 main 分支上跑通了一版 harness，但工程评审认为 **过度工程化**（critic / plan resolution lifecycle / 4-persona 子代理 / decision draft 流水线 / FMP 财报），结论：

- 大部分深度投研机制是为"机构级审计"目标服务的，但 **当前优先级不是这个**，且这套机制在简单任务上扭曲了 agent 行为。
- 正确做法是 **回到一个最小、正确的 agent loop**，先把单一市场（A 股）做扎实，再按需逐项加回严格机制。

`rebuild` 分支的 milestone 安排：

1. **Phase 0 — 瘦身**：删 critic / decision_draft / record_investment_action / use_skill / delegate_research；把 plan resolution lifecycle 退化成 checkbox 风格的简单 markdown plan；skill 临时静态注入，等 M2.5 升级。
2. **Milestone 1 — Agent loop MVP**：单 turn 级别的 cost cap、wall-clock 超时、tool error 连续上限、doom loop 检测、observability（`task_metrics`）、plan movement detector。（这几条 Claude Code 和 codex-rs 都没完整做，leek 是要更严。）
3. **Milestone 2 — Corpus MVP**：corpus panel 接 live、corpus 内编辑、corpus_read 卡片打磨。
4. **Milestone 2.5 — Skill / hook / plugin**：照抄 Claude Code（SKILL.md + YAML frontmatter、lazy load、热重载；shell command hook 配 stdin/stdout JSON；`.leek-plugin/plugin.json` 清单）。
5. **Milestone 3 — A 股 MVP**：5–7 个核心工具（行情、K 线、资金流、财报、公告、行业、宏观），端到端跑通三类任务（单股估值 / 行业研究 / 资金筹码）。
6. **Milestone 4 — A 股完整**：工具覆盖度、复杂场景、设时间盒。

多市场扩展（美股、crypto）和 TUI 都放在 A 股做扎实之后。

### 技术栈

- **Gateway**：Rust（`crates/gateway/`）—— Axum HTTP、SQLx + SQLite、SSE 事件协议。
- **前端**：SolidJS + Vite（`frontend/web/`）—— chat / canvas / corpus brain / plan widget。
- **Vault**：单文件 SQLite —— sessions、messages、events、tasks、plans、decisions、corpus refs。
- **Corpus**：git submodule [`hchen13/the-corpus`](https://github.com/hchen13/the-corpus) —— 只读投研知识库；通过 `corpus/inbox/` 工作流晋升新内容。
- **LLM**：OpenAI Responses API（codex 后端），多 provider 抽象在 `crates/gateway/src/llm/`。

### 快速开始

> 依赖：Rust ≥ 1.80、Node ≥ 20、`pnpm` / `npm`、OpenAI API key（codex OAuth），可选 Tushare Pro key 跑 A 股财报。

```bash
# 含 corpus submodule clone
git clone --recursive https://github.com/hchen13/leek.git
cd leek

# 把 corpus distill 成 prompt 资源（每次 corpus 版本变更跑一次）
cargo run --bin leek -- corpus distill --root ./corpus

# 起 gateway
cargo run --bin leek -- --vault ./vault.db serve --port 8964

# 另开一个终端起前端
cd frontend/web
npm install
npm run dev
# → http://localhost:5173
```

每用户配置走 `mandates/<user_id>.md` —— 风险偏好、单票上限、可交易工具限制。Agent 每个 turn 重读，不需要重启。

### 文档入口

- `AGENTS.md` —— 设计纪律总览（先读这个）。
- `design/decisions/` —— 设计决策 ADR（`0001..0012`）。
- `design/p1-spec/` —— 模块契约（agent loop、API、corpus brain、data schema、tools）。
- `harness/discipline.md` —— Agent 行为准则（system prompt 里被引用）。
- `harness/skills/<name>/SKILL.md` —— 任务类型对应的 framework（当前有 `equity-valuation`、`crypto-research`）。

### 贡献

项目正在 active rebuild。Issue / PR 欢迎，但 M3 之前会有大量 churn。`main` 分支冻结在 sprint 末快照；当前所有工作在 `rebuild` 分支。
