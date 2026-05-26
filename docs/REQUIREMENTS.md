# L.E.E.K 需求说明

> 本文面向后续接手开发的 agent。它是独立需求说明：读者不需要知道任何项目历史，也不需要再查其它设计文档才能理解产品边界、系统形态和阶段验收。
>
> 当前日期：2026-05-19。

## 0. 文档权威性

本文是 L.E.E.K 的产品与 UX 合同。它回答“系统应该是什么、用户如何使用、前端如何呈现 agent 工作”。

权威顺序：

1. 当前代码：实现事实以代码为准。
2. 本文：产品边界、UX 边界、验收场景以本文为准。
3. `docs/ARCHITECTURE.md`：端态架构原则。
4. `docs/MILESTONES.md`：阶段顺序与已完成状态。

旧 `design/` 目录只作为历史参考，不是当前需求源。任何 `task` 实体、deliverable 分类、user mandate、portfolio/holdings、plan guard、`LlmProvider` trait、Reasoning DAG 等旧路线，除非本文重新引入，否则都不得复活。

术语：

- **turn**：一次用户输入到一次最终 assistant 回复。
- **iteration**：一个 turn 内的一次模型调用。
- **canvas artifact**：由事件流驱动的可视化过程卡片，如 note、tool、search、corpus、subagent。
- **auto-compaction**：上下文接近窗口上限时，先生成可追溯摘要替换长历史，然后继续同一个 turn。这是上下文接近上限时的**唯一**设计行为——不存在"停下来让用户重开 session"的兜底分支。

---

## 1. 产品定义

L.E.E.K 是一个面向投资研究的本地 AI agent 工作台。

用户主要通过 chat 指挥 agent；agent 调用工具、检索 corpus、读取网页、查数据、维护计划，并把工作过程展示在 canvas 和右侧信息栏中。用户在工作台上主要做三件事：

- 向 agent 提出投研问题、追加约束、回答澄清问题。
- 查看 agent 查到的资料、工具结果、corpus 引用和执行轨迹。
- 基于最终回复做自己的投资判断。

L.E.E.K 不是投研 dashboard，不是交易系统，也不是一组 prompt 模板。它的核心是一个现代 agent loop，加上投资 corpus、投研工具和可溯源的工作台界面。

### 1.1 核心差异化

L.E.E.K 和通用 agent 的差异不在 loop 原语，而在内容和呈现：

- **投资 corpus**：用户维护的 wiki 层知识库，是 agent 的投研认知底座。
- **投研工具**：行情、K 线、财务、公司信息、资金流、网页读取等工具。
- **可见工作台**：用户能看到 agent 使用了哪些资料、哪些工具、哪些 corpus 节点。

### 1.2 成功标准

系统成功时应满足：

- 简单问题被简单处理，不自动扩成大型调研。
- 复杂问题可以被 agent 自主拆解、查证和综合。
- 用户能在 canvas 中追溯 agent 的执行过程。
- 工具失败、超时、数据源异常不会被完全隐藏。
- 最终回复只在 chat 中呈现，清楚、可读、回到投资判断本身。

---

## 2. 主界面

主界面是 chat-led research workbench。

```text
+----------------------+-----------------------------------+----------------------+
| Chat                 | Canvas                            | Corpus Brain          |
|                      |                                   |                      |
| user messages        | turn sections                     | full wiki graph       |
| final replies        | note trace                        | session activation    |
| tool/progress summary| tool/search/corpus cards          |                      |
| composer             | subagent cards                    +----------------------+
|                      |                                   | Plan / TODO           |
+----------------------+-----------------------------------+----------------------+
```

### 2.1 Chat

Chat 是用户的主输入和最终回复区域。

必须支持：

- session 创建、切换和历史加载。
- 用户自然语言输入。
- assistant 最终回复流式展示。
- 当前 turn 的运行状态。
- 当前 turn 的极简工具 / 进度摘要。
- 运行中逐条显示 tool / search / subagent 状态。
- turn 完成后把工具摘要折叠成聚合行，并允许展开。
- 点击工具摘要项时，滚动并高亮 canvas 上对应卡片。

Chat 不显示 note 正文。note 属于 canvas。

运行中工具摘要粒度：

```text
▸ 查行情 · 600519.SH
✓ 读 corpus · margin-of-safety
✗ 查财务 · 600519.SH
```

要求：

- 每个 tool call 一行。
- 使用 user-friendly display name。
- 失败项必须显示。
- 成功项点击跳转 canvas 卡片。
- 失败项如果 canvas 默认隐藏，则点击打开失败详情。
- turn 完成后聚合，例如：`已执行 8 步 · 1 个失败 · 3 个数据卡片`。

### 2.2 Canvas

Canvas 是只读为主的执行轨迹工作区。

它展示 session 级累积轨迹，并按 turn 分段。每个 user turn 对应一个 canvas section。长 session 中历史 turn 可以折叠；折叠态至少显示：

- 用户问题摘要。
- 工具数量。
- 失败数量。
- 是否有 note。
- 耗时。
- 停止原因。

Canvas 分段内按事件线性展示：

- Note Trace card。
- tool card。
- provider-side search card。
- corpus card。
- subagent card。

Canvas 不展示最终回复 card。最终回复只在 chat 中展示。

Canvas 不是用户自由编辑的白板，也不是 dashboard。用户在 canvas 上的交互原则上只用于查看：

- 展开详情。
- 打开 modal。
- 缩放 / 平移图表。
- 切换图表周期。
- 查看失败 debug 信息。

这些交互默认不写状态、不产生副作用、不回灌给 LLM。如果用户希望 agent 基于某张卡片继续分析，必须通过 chat 或明确的回答输入触发新的 agent turn。

### 2.3 Note Trace

Note Trace 是 assistant 在工具调用前后产生的可展示说明。它来自模型响应中的普通 content/text block：

- 当前 Codex / OpenAI Responses API：与 tool calls 平级的可见 `content`。
- 未来如果接入其它 provider，再把其可见 text block 映射到同一语义。

Note Trace 不是 hidden thinking，不是 chain-of-thought，不是模型内部推理原文。

展示规则：

- note 只在 canvas 显示。
- 相邻 notes 合并为同一张 Note Trace card。
- 被 tool/search/subagent card 隔开的 notes 不合并。
- note 与 tools 是线性关系，不是父子层级。

### 2.4 Tool Cards

每次 tool call 默认产生一张 tool card。只有被判定为同一 canvas identity 的调用才合并 / 更新。

默认 canvas identity：

```text
tool_name + normalized_args
```

每个 tool 可以覆盖 canvas identity。例如：

- K 线工具可把同一标的不同 interval / time range 映射到同一张可交互 K 线卡片。
- 不同标的必须是不同卡片。

失败 tool call：

- canvas 主轨迹默认不显示失败 tool card。
- turn 摘要显示失败计数。
- chat 工具摘要必须显示失败项。
- canvas settings 打开“显示失败工具调用”后，失败卡按原时间位置出现。

### 2.5 Corpus Brain

右上角常驻 Corpus Brain。

语义：

- 显示全 wiki graph。
- 当前 session 触发过的 wiki 节点叠加激活状态。
- 当前 turn / 当前工具触发的节点使用更强或更短暂的 live activation。
- 历史激活保留为较弱状态。

Corpus Brain 图谱范围只覆盖 wiki pages，不覆盖 sources。

Corpus Brain 的第一价值不是普通文件浏览，而是让用户看到 corpus 如何参与 agent 的工作。

### 2.6 Plan / TODO

右下角显示 agent 显式维护的 plan / TODO。

要求：

- 只有存在 plan 时才显示。
- 没有 plan 时不显示该区域。
- 不从 runtime 自动推导一个假进度。
- `update_plan` 不进入 canvas tool card。
- plan 只是展示层，不是 gate；不会因为 plan 未完成而阻止 agent 回复。

---

## 3. Agent 行为

### 3.1 主 Agent

每个活跃 session 有一个主 agent loop。一次用户输入到一次最终 assistant 回复称为一个 turn。

主 agent 自己决定：

- 何时直接回答。
- 何时调用工具。
- 何时检索 corpus。
- 何时读取网页。
- 何时维护 plan。
- 何时 spawn subagent。
- 何时停止并回复。

禁止默认引入：

- 上游意图分类模型。
- 固定产物分类器。
- 固定任务状态机。
- plan guard。
- 强制产物类型。

### 3.2 System Prompt 组成

system prompt 按稳定性从高到低组织：

1. 产品身份：L.E.E.K 是什么。
2. Corpus orientation：少量稳定的 corpus 使用原则，目标小于 800 tokens。
3. 工具清单：工具名和极短描述。
4. Skill 索引：每个 skill 只列 name 和 description。

不写入：

- 大段“什么时候用哪个工具”的规则。
- 固定产物框架。
- “必须先做计划”的强制要求。
- UI display metadata。

### 3.3 Subagent

subagent 是上下文隔离和并行委派机制，不是 persona 类型系统。

要求：

- 主 agent 通过 `task` 工具 spawn subagent。
- subagent 使用同一套 loop 和 guard。
- subagent 有自己的 system prompt、工具子集、context window。
- subagent 最多嵌套 depth=2。
- subagent 返回 text block 给父 agent。

配置形态：

```yaml
---
description: <这个 agent 能完成什么委派工作>
allowed_tools: [...]
model: <optional>
---

<system prompt body>
```

发现路径：

- 内置 agents。
- 用户全局 agents。
- 项目级 agents。

同名优先级：项目级 > 用户全局 > 内置。

内置 agents 至少包括：

- `general-purpose`：默认通用 worker，全工具集。
- `corpus-expert`：只使用 corpus search/read，负责 corpus-grounded synthesis。

不做 planner subagent。计划由主 agent 通过 `update_plan` 就地维护。

#### Subagent UI

如果 agent spawn subagent：

- canvas 显示 subagent card。
- 默认折叠。
- 折叠态展示 agent 名、子任务、状态、耗时、工具数、结果摘要、失败信息。
- 可展开。
- 展开后显示该 subagent 内部 note trace 和 tool use 轨迹。
- chat 工具摘要显示 subagent 启动 / 完成 / 失败，并能跳转到对应 card。

---

## 4. Tool 系统

### 4.1 LLM Tool Spec 与 UI Metadata 分离

给 LLM 的 tool spec 只包含：

- 内部稳定 `name`。
- LLM-facing description。
- JSON schema。

前端渲染 metadata 是 UI-only，不进入 LLM 上下文：

- `display_name`
- `summary(args, status, result?)`
- `canvas_identity(args)`
- `card_kind`
- renderer 选择

同一个工具的 LLM spec 和 UI metadata 可以在代码上相邻注册，但语义上必须分开。

### 4.2 Tool Result Surfaces

client-side function tool 执行后返回三个面向：

```text
model_output      给 LLM 继续推理，进入 function_call_output。
display_payload   给 UI 渲染卡片，不进入 LLM 上下文。
debug_payload     给展开详情 / 开发调试使用。
```

要求：

- tool 自己生成结构化 `display_payload`。
- agent loop 不理解业务数据，不做 UI payload 转换。
- 前端不从 `model_output` 文本里解析业务数据。
- `model_output` 和 `display_payload` 不要求一致；两者用途不同。
- 两者必须来自同一次 tool execution。

示例：K 线工具

- `model_output` 可以是固定 interval / range 的摘要或紧凑数据。
- `display_payload` 可以包含用于可交互图表的数据。
- 用户在 canvas 中缩放、平移、切周期，只影响用户视图。
- 如果用户要 agent 基于新范围继续分析，必须触发新的 agent turn / tool call。

### 4.3 Provider-side Search

Web search 使用 provider/server-side search，不作为普通 client-side function tool 自己实现。

要求：

- 请求中启用 provider 的 `web_search` tool。
- 解析 provider 的 search lifecycle event。
- 规范化成统一的 canvas artifact event。
- search card 展示 query batch 和 sources。
- 如果 provider 只给 URL，则显示 URL / host。
- 如果 provider 给标题 / 摘要，则显示标题 / 摘要。
- 不为了补齐标题摘要自动扩大 agent 工作流。

`web_fetch` 是独立 client-side tool，用于读取指定 URL 的正文，不替代 search。

### 4.4 基础工具

必须具备：

- `update_plan`：维护右下角 plan / TODO。
- `corpus_search`：搜索 wiki。
- `corpus_read`：读取 wiki/source page。
- `web_fetch`：读取指定 URL。
- provider-side `web_search`：搜索和打开网页。

投研纵向工具包括但不限于：

- `market_quote`
- `get_candlesticks`
- `get_financials`
- `get_company_info`
- `get_capital_flow`
- `get_industry_peers`（M4.1）
- `get_business_breakdown`（M4.1）
- `get_announcements`（M4.1）
- `get_consensus`（M4.1）
- `get_top_holders`（M4.1）
- `get_concepts`（M4.1）

M4.1 起的 6 个工具在所有 vendor 都拒绝时 **必须** 返回结构化的
`data_available: false`，模型基于此明示用户"该项不可用"，**绝对禁止**
凭印象补数。

工具名称是否最终采用以上名字，由实现时的工具语义决定；UI 必须通过 display metadata 渲染人话名称。

---

## 5. Corpus

### 5.1 Corpus 结构

corpus 有一个 configured corpus base / vault root，作为安全边界和索引入口。

page 存在于 registry 中，而不是任意文件路径。registry 覆盖：

```text
wikis/**/*.md
sources/**/*.md
```

page id 是 registry id。

### 5.2 Search 与 Read

`corpus_search`：

- 只搜索 wiki pages。
- 只返回 wiki page id。
- 不返回 source page。

`corpus_read(id)`：

- 可读取 registry 中合法的 wiki page 或 source page。
- 拒绝 registry 外 id。
- 拒绝 path traversal。

source 的发现路径：

- wiki 页面中的引用 / link。
- 用户显式提供 source id。

### 5.3 Corpus Cards

`corpus_search` / `corpus_read` 在 canvas 中显示 wiki page 的 markdown 缩略渲染。

点击后打开完整 markdown modal。

source page 可以在 modal 或引用列表中打开，但不进入 Corpus Brain 主图。

---

## 6. State 与 Storage

每个用户一份 local vault。初始持久状态只包含当前必须状态。

基础表：

- users
- sessions
- messages
- events
- turn_metrics
- auth token storage

当前不存：

- 用户消息预分类后的工作流实体。
- 固定产物实体。
- portfolio / holdings。
- user mandate / memory。
- provider_configs 抽象表。
- plans 表。
- subagents 表。

说明：

- 会话就是消息序列和事件序列。
- plan 通过事件和当前内存状态展示，不需要独立核心表。
- subagent 可以通过 turn_metrics parent linkage 或事件关联观测，不需要独立核心表。
- 每张新增表必须说明为什么当前阶段必须加入。

---

## 7. Guard 与 Observability

### 7.1 Guard

默认 guard：

- idle timeout：90 秒。
- wall-clock 上限：30 分钟。
- doom-loop detector：连续相同 tool+args 达到阈值时停止。
- auto-compaction threshold：90% context window。
- per-turn metrics：默认开启。

Opt-in guard：

- iteration cap。
- cost cap。

wall-clock 软提示：

- 剩 10 分钟：考虑收窄分析范围。
- 剩 5 分钟：开始组织最终回答。
- 剩 2 分钟：完成已在跑的工具，不开新分支。
- 剩 1 分钟：立刻收尾，用已有信息给结论。

Guard 触发时不能静默失败，必须产出可见停止原因和已有部分结果。

#### Auto-compaction

上下文接近窗口上限时，系统**自动压缩并继续**，不停下来要求用户重开 session。

- 检测到上下文接近上限时，先把长历史压缩成一个可追溯摘要，保留目标、约束、关键证据、工具结果、未完成分支和 provenance，然后用摘要替换长历史，继续同一个 turn。
- 这是上下文接近上限时的**唯一**设计行为。不设计“压缩失败就停 turn”的护栏分支：压缩本身是一次模型调用，如果它失败，就走和任何模型调用失败一样的通用错误路径，不需要一个专门的停止状态。
- context window 与 90% 触发阈值都是配置项，默认对齐 codex（gpt-5.5 经 codex 后端是 272K，阈值 90%），可分别通过 `LEEK_CONTEXT_WINDOW` / `LEEK_AUTO_COMPACT_THRESHOLD` 覆盖。

当前代码里有一个 `context_limit` 停止分支（到阈值就结束 turn 并给诊断）。**这不是本产品的设计**，是开发过程中被擅自加入的工程护栏。M1 的 auto-compaction 必须替换掉它，不是在它旁边并存。

### 7.2 Observability

每个 turn 必须记录：

- turn_id
- session_id
- model
- started_at / ended_at
- wall_clock_ms
- iteration_count
- tool_call_count
- tool_error_count
- input_tokens
- output_tokens
- cost estimate
- stop_reason
- first_triggered_guard
- fatal_error

事件流最终必须覆盖下列语义；对应功能未落地前不要求伪造事件：

- message created
- assistant delta
- note trace event
- tool call start / completion / error
- provider-side search lifecycle
- plan updated
- subagent start / progress / completion / error
- assistant done
- turn metrics recorded
- compaction started / completed
- error

---

## 8. 技术形态

### 8.1 后端

后端是单 gateway 服务，承担：

- HTTP API。
- SSE stream。
- SQLite vault。
- agent loop。
- tool registry。
- corpus registry / search / read。
- auth token storage。

实现上优先保持单 crate / 单服务，除非出现真实拆分理由。

### 8.2 前端

前端是 web workbench，承担：

- chat。
- canvas artifact stream。
- Corpus Brain。
- plan widget。
- markdown modal。
- chart / table / preview card rendering。

前端不负责判断工具语义，不从 LLM 输出中解析业务数据。前端只渲染 `display_payload` 和 UI metadata。

### 8.3 LLM 访问

默认只有一条模型访问路径：codex OAuth → Responses API。

不抽象多 provider。只有当第二条真实 provider 路径出现，并且契约明确时，才引入抽象层。

---

## 9. 阶段验收

完整验收范围由以下阶段组成。每个阶段完成后，系统都必须处于可运行状态。

### M0 — Plumbing Skeleton

目标：证明最小 HTTP / SQLite / SSE 闭环。

包含：

- gateway 启动。
- SQLite 初始化。
- health endpoint。
- session create/list。
- message post/list。
- SSE event stream。
- echo response。

不包含：

- LLM 调用。
- agent loop。
- corpus。
- skill。
- subagent。
- 投研工具。
- 产品工作台 UI。

验收：

- 创建 session。
- 发送一条消息。
- SSE 收到 assistant echo。
- message 和 event 持久化。
- 服务重启后仍可读取历史。

### M1 — Agent Loop

目标：真实主 agent loop 端到端跑通。

包含：

- codex OAuth。
- Responses streaming。
- 主 agent loop。
- client-side function tool dispatch。
- `echo` / `web_fetch`。
- turn_metrics。
- guard set。
- auto-compaction。
- chat 流式最终回复。

验收：

- 用户发消息后触发真实模型调用。
- 模型可以调用工具。
- 工具结果进入下一轮模型输入。
- 最终回复在 chat 中流式出现。
- guard 触发时有停止说明。
- auto-compaction 到阈值时生成可追溯摘要并继续同一个 turn；这是上下文接近上限时的唯一行为。

当前实现状态：M1 主 loop / 工具循环 / guard / turn_metrics 已完成；auto-compaction 尚未完成——当前代码用的是一个 `context_limit` 停止分支（开发过程中被擅自加入的护栏），M1 收尾必须用 auto-compaction 替换它。M1 也不包含完整 workbench canvas UX 或 provider-side `web_search`。

### M1.9 — Workbench Event Contract

目标：把 M1 后端事件升级成前端 workbench 可稳定消费的事件契约。

包含：

- `note_trace` 事件。
- tool call start / completion / error 的统一事件。
- `canvas_artifact` 基础 envelope。
- tool UI metadata registry。
- `model_output / display_payload / debug_payload` 三分法。
- chat 工具摘要运行中逐条显示，完成后聚合。
- 基础 canvas：按 turn 分段，展示 note/tool/web_fetch cards。
- provider-side `web_search` 事件解析和 search card，如果当前 Codex backend 可用。
- `update_plan` tool 和 `plan_updated` 事件的最小实现。

验收：

- note 不进入 chat 正文，进入 canvas。
- 最终回复只在 chat 中出现。
- tool card 不从 `model_output` 文本解析业务数据。
- 失败工具在 chat 摘要可见。
- `update_plan` 只更新右下角 Plan / TODO，不进入 canvas tool card。

### M2 — Corpus

目标：让 agent 能使用投资 corpus。

包含：

- corpus registry。
- wiki-only lexical search。
- wiki/source read。
- corpus orientation 注入 system prompt。
- corpus cards。
- markdown modal。

验收：

- `corpus_search` 只返回 wiki page id。
- `corpus_read` 可以读取合法 wiki/source page。
- source 不出现在 search 结果中。
- corpus card 可打开完整 markdown modal。

### M2.1 — Corpus Brain UI

目标：让用户看到 corpus 如何参与 agent 的工作，而不是只把 corpus 当文件列表。

包含：

- Corpus Brain 全 wiki graph。
- session activation overlay。
- 当前 turn / 当前工具的 live activation。
- 历史 activation 的弱化保留。

验收：

- Corpus Brain 显示全 wiki graph。
- agent 使用 corpus 时对应节点激活。

### M2.5 — Skill / Hook / Plugin

目标：引入扩展机制。

Skill：

- 三层发现：内置、用户全局、项目级。
- system prompt 只列 description。
- body 通过 `use_skill(name)` 懒加载。

Hook：

- 绑定事件。
- shell command 执行。
- 超时。
- stdout / exit code 捕获。

Plugin：

- skill + hook + manifest 的本地 bundle。
- 支持本地安装。

验收：

- skill index 进入 system prompt。
- `use_skill` 可加载 body。
- hook 可在指定事件执行。
- plugin 可作为本地 bundle 安装。

### M2.7 — Subagent

目标：提供 context isolation 和并行委派。

包含：

- `task` tool。
- AGENT.md loader。
- subagent loop spawn。
- depth 上限。
- parent turn linkage。
- `general-purpose`。
- `corpus-expert`。
- subagent canvas card。

验收：

- 主 agent 可 spawn subagent。
- subagent 使用同一套 guard。
- subagent card 默认折叠，可展开看内部 tool use。
- chat 摘要显示 subagent 状态。
- depth 超限时有清晰错误。

### M3 — A 股研究纵向

目标：落地第一个完整投研工具纵向。

包含：

- 行情快照。
- K 线。
- 财务数据。
- 公司信息。
- 资金流。
- 工具卡片结构化 display payload。
- 快速扫描。
- 深度复盘。
- 多标的对比。

验收：

- 常见 A 股问题可以端到端完成。
- K 线卡片可交互，UI 操作不自动回灌 LLM。
- 财务 / 公司 / 资金流卡片结构化渲染。
- 多标的对比可并行委派。
- 简单问题不会膨胀成无边界调研。

### M4 — 投研工作台完整化

目标：让系统进入日常投研可用状态。

包含：

- 常见研究问题覆盖。
- 更完整的工具错误展示。
- 更稳定的 long-session canvas 折叠 / 回放。
- 更完整的 eval set。
- 可观测性面板。

不强行包含：

- 自动交易。
- paper trading。
- portfolio 管理。
- 用户 memory / mandate。

这些属于独立课题，只有当真实使用暴露明确需求后再设计。

---

## 10. 非目标

明确不做：

- 自动下单。
- paper trading。
- 投资组合持仓管理。
- 用户 mandate / memory 半成品。
- 固定产物类型系统。
- canvas 结论卡片。
- 独立任务数据库实体。
- plan guard。
- 上游意图分类模型。
- hardcoded persona subagent。
- 多 provider 抽象。
- 让 UI metadata 进入 LLM 上下文。
- 从 tool 文本输出中解析业务数据作为主要 UI 契约。

---

## 11. 验收场景

### 11.1 简单解释

输入：

```text
margin of safety 是什么意思？
```

期望：

- 可直接回答，或轻量使用 corpus。
- 不启动大型调研。
- 最终回复在 chat。
- 如果使用 corpus，canvas 出现 corpus card，Brain 激活相关节点。

### 11.2 Corpus-grounded 回答

输入：

```text
用我们的 corpus 解释一下芒格怎么看能力圈。
```

期望：

- `corpus_search` 搜 wiki。
- `corpus_read` 读取相关 wiki。
- 如需原文证据，再读取 wiki 引用的 source。
- canvas 显示 corpus card。
- markdown modal 可打开全文。

### 11.3 Web Search

输入：

```text
查一下最近 AI capex 的讨论，哪些来源值得继续读？
```

期望：

- 使用 provider-side search。
- canvas search card 展示 query 和 source URL。
- 如果 provider 给标题 / 摘要则展示；没有则显示 URL / host。
- chat 工具摘要显示 search 状态。

### 11.4 Web Fetch

输入：

```text
读一下这个链接，提炼和 GPU 需求相关的要点：<url>
```

期望：

- 使用 `web_fetch`。
- canvas 显示网页缩略图 / link preview。
- 失败时 chat 工具摘要显示失败项。
- 失败不会在 canvas 主轨迹默认出现。

### 11.5 K 线分析

输入：

```text
看一下 600519.SH 最近走势有没有异常。
```

期望：

- agent 调用 K 线工具。
- model_output 给 agent 当前分析所需数据。
- display_payload 渲染交互式 K 线卡片。
- 用户可缩放、平移、切周期。
- 用户操作不自动进入 LLM 上下文。

### 11.6 Subagent

输入：

```text
对比 BABA、PDD、JD，谁更值得进入深度研究？
```

期望：

- 主 agent 可委派多个 subagent。
- canvas 出现多个 subagent card。
- 默认折叠。
- 展开可看内部 note trace 和 tool cards。
- chat 摘要显示每个 subagent 状态。

### 11.7 失败透明度

输入：

```text
分析某只股票，期间财务 API 连续失败。
```

期望：

- chat 工具摘要显示失败项。
- turn 完成后聚合摘要保留失败计数。
- canvas 默认隐藏失败 card，但可通过 settings 显示。
- final reply 必须承认关键数据失败对结论的影响。

---

## 12. 设计原则

- Agent 自主优先：不要用确定性状态机替代模型决策。
- 可观测性默认开启：非确定性系统必须能追踪。
- UI 只读为主：工作台展示 agent 工作，不制造隐式副作用。
- Corpus wiki first：search 找 wiki，read 取证。
- 工具语义归 tool：tool 自己生成 display payload。
- Chat 保持干净：对话和最终回复在 chat，过程细节在 canvas。
- Plan 是展示，不是闸门。
- 简单任务不能膨胀。
- 只有真实需求出现时才增加持久状态和抽象层。
