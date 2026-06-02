[English](README.EN.md) | [中文](README.md)

# L.E.E.K（老韭菜）

**Logic-Enhanced Equity Kernel** 是一个独立开发的金融投研 agent 产品。它把大语言模型、投资原则语料库、市场数据工具、网页研究、画布化证据和可追踪的 agent harness 放在同一个本地工作台里，目标是让一次投资研究从“随口问问”变成“有框架、有证据、有反方、有行动约束”的完整流程。

![L.E.E.K web workbench](docs/assets/leek-workbench.png)

L.E.E.K 不是普通聊天机器人。它的核心不是把工具结果拼成一段看似专业的回答，而是让 agent 在研究过程中持续暴露：它为什么这样查、查到了什么、还缺什么、如何从事实走向判断。聊天窗口保留简洁对话，画布承载工具卡片、推理轨迹、财务表格、K 线、网页证据和 corpus 激活状态。

当前项目仍处在早期 alpha。流程已经能跑，但还在持续加固分析质量、工具质量、前端体验和长程任务可靠性。它不是投资建议系统，也不替代个人判断、持牌投顾或正式风控流程。

## 产品目标

L.E.E.K 面向想严肃做投研的个人投资者和研究者。它试图解决几个普通 LLM agent 很容易失败的问题：

- 简单汇总数据，却没有真正建立研究框架。
- 忘记上一轮已经查过的事实，重复调用同一批工具。
- 直接给“买/卖”结论，却没有说明永久性亏损风险、能力圈边界和反方证据。
- 把网页搜索、财务数据、行情和投资原则割裂成互不相干的信息片段。
- 长程任务中跑偏、过早停下，或者把未完成的问题交还给用户。

L.E.E.K 的目标是成为一个能持续工作的投研 harness：让模型更像研究员，而不是只会输出研报腔文字的聊天框。

## 核心能力

- **本地 agent gateway**：Rust 编写的长运行服务，负责 session、事件流、工具调用、vault 状态和 LLM provider。
- **Chat-canvas 工作台**：左侧聊天，中央画布展示证据和产物，右侧展示 corpus brain、计划和当前上下文。
- **A 股优先的数据工具**：支持公司信息、财务、行情、K 线、资金流、行业、指数、基金、宏观等 A 股研究入口。
- **Corpus grounding**：从 curated investing corpus 中检索原则、实体和来源材料，把 Buffett / Munger / Dalio 等底层框架转成研究约束。
- **Reasoning trace**：把 agent 的阶段性研究意图以 UI 事件展示出来，让用户能看到它如何推进任务。
- **Plan 与 subagent 雏形**：长程任务可以创建计划、委派子研究任务，并把进展写回 session。
- **Append-only session vault**：对话、工具调用、计划、LLM usage 和画布事件写入本地 SQLite，便于复盘、调试和评测。
- **Prompt cache 优化**：面向 Codex backend 的 session identity 与 cache key 已接入，长工具循环的 cache hit rate 已显著提升。

## 架构概览

```text
frontend/web        SolidJS + Vite web workbench
crates/gateway      Rust gateway, CLI, API, agent loop, tools, vault
corpus/             投资原则与知识语料，作为 read-mostly knowledge layer
harness/            agent identity、discipline、corpus orientation
tests/              A 股 E2E eval cases 与测试记录
design/             架构、决策记录和历史设计材料
```

运行时主要由四层组成：

1. **Agent harness**：决定如何构造上下文、何时调用工具、如何追踪计划、如何恢复 provider error。
2. **Tool layer**：把市场数据、网页、corpus、财务和研究来源包装成少数高杠杆工具。
3. **Vault**：保存每个 session 的事件、消息、工具调用、plan 和 provider 配置。
4. **Workbench UI**：把聊天、推理、工具证据和 corpus 状态组织成可阅读的投研界面。

## 本地运行

安装依赖后，先构建 gateway：

```bash
cargo build -p leek-gateway
```

首次使用需要配置 Codex provider：

```bash
cargo run -p leek-gateway -- --vault tmp/dev/vault.db auth codex
```

启动 gateway：

```bash
cargo run -p leek-gateway -- --vault tmp/dev/vault.db serve --port 8964
```

另开一个终端启动前端：

```bash
npm --prefix frontend/web install
npm --prefix frontend/web run dev -- --host 127.0.0.1 --port 5173
```

然后打开：

```text
http://127.0.0.1:5173
```

如果只想做一次 provider smoke test：

```bash
cargo run -p leek-gateway -- --vault tmp/dev/vault.db chat "用一句话介绍 L.E.E.K"
```

## 数据与配置

- `--vault` 指向本地 SQLite vault，保存用户运行时状态。
- A 股数据优先使用已配置的数据源；Tushare token 等 provider 凭据应放在本地配置或应用 settings 中，不应提交到仓库。
- `corpus/` 是通用投资知识层。agent 默认读取它，不应直接把 session 临时结论写入正式 corpus。
- `tmp/` 用于本地临时文件、测试 vault 和实验脚本。

## 开发检查

```bash
cargo check -p leek-gateway
cargo test -p leek-gateway
npm --prefix frontend/web run build
```

固定 A 股 eval cases 位于：

```text
tests/a_share_e2e_cases.md
```

这些 case 用来观察三类问题：harness 是否可靠、agent 表现是否成熟、前端是否正确展示工具证据与时序。

## 当前状态

已经具备的基础：

- session / event log / SSE / canvas 基本跑通。
- agent loop 能调用工具，并保留工具卡片与 tool_call_runs。
- plan、subagent、provider retry、prompt cache、corpus brain 均已有可运行版本。
- A 股公司、财务、行情、K 线、资金、行业、宏观等工具已有第一版。
- 前端可以展示 reasoning trace、工具卡片、plan、corpus brain 和 settings。

仍在重点打磨：

- 分析质量还需要从“工具结果汇总”升级到真正稳定的投研方法论。
- subagent 还不是成熟可靠的 worker 系统。
- A 股数据层仍需继续补齐分钟线、实时行情、研报、公告、行业和替代资金面。
- 前端卡片、财务详情、K 线交互、画布布局和性能还在持续迭代。
- E2E eval 需要长期跑通、记录问题、批量修复并回归。

## 项目原则

- L.E.E.K 不预定义机械输出模板；输出应由任务、证据和用户约束自然决定。
- 简单问题不强制 plan；复杂长程任务才使用 plan 来防止跑偏。
- 工具应少而高杠杆，从 agent 视角设计输入、输出和错误反馈。
- Corpus 是思维框架，不是答案库；没有命中 knowledge 时，agent 仍应通过研究建立本轮 working model。
- 任何最终动作都必须尊重用户的仓位、风险偏好和“永久性亏损”约束。

## 免责声明

L.E.E.K 是研究工具，不提供投资建议。所有输出都可能出错，所有市场数据都可能延迟、不完整或来自第三方。任何投资决策都应由用户自行判断并承担风险。
