# L.E.E.K 架构设计

> **rebuild-clean 分支的端态规格说明。**
>
> 配套文档：`docs/MILESTONES.md`。MILESTONES 告诉你*什么时候发布什么*；
> 本文档告诉你*全部里程碑落地后系统长什么样*。两份保持同步——本文档
> 里某个架构决策有变动时，找到对应的 milestone 一起更新。
>
> 最近修订：2026-05-11，rebuild-clean 重置时。

---

## 1. 项目目标

L.E.E.K 是一个**面向投资研究领域的专用 AI agent**。

结构上它几乎就是 codex / Claude Code 的克隆：

- 一个主 agent loop
- 一套工具注册表（tool registry）
- 一小撮内置 skill
- 现代 harness 标配的三件套：skill / hook / plugin
- 一个 subagent spawn 机制，用于把任务委派给专用子 agent

**差异化不在结构上**，而在内容上：

- 一份精心维护的投研 **corpus**（markdown 知识库，注入主 agent 的
  system prompt，也可以通过工具查询）
- 一组**领域工具**（行情/财务/资金流/资讯查询）
- **用户 mandate**——用户的持仓、风险偏好、风格、时间维度——
  收集一次后跨 session 持久化

除此之外都跟业内已经收敛的 agent harness 模式一致。我们不发明
agent loop 原语。我们采纳它们。

---

## 2. 明确的"不做"清单

下面这些在过去版本的 leek 里有，rebuild-clean 里**故意没有**：

- **不做 routing 层。** 每条用户消息直接进 main loop。不会有一个
  上游 LLM 把消息分类成 `new_task` / `chat_reply` / `ambiguous`。
  是否调用工具由模型自己决定。
- **不做 deliverable 分类。** 没有 `research_brief` / `comparison`
  / `morning_brief` / `free_form` 这种分类。用户问什么，模型就
  产出什么形态的回答。
- **vault 里不存 `task` 实体。** 会话就是消息序列。会话和消息之间
  不再有一个 LLM 分类出来的"task"记录。
- **不做 `plan_guard` 强制。** 模型想用 `update_plan` 就用，觉得
  答案 ready 了就交付。不会出现"plan 没走完不准交付"的拦截。
- **不把 subagent 写成硬编码的 Rust persona 类。** 多个专业化子
  agent（`corpus-expert` / `planner` / `market-data-fetcher` 等）
  是想要的；但它们的 system prompt 和工具子集通过**外部配置**
  （skill 文件之类）提供，而不是 Rust 里的 hardcoded persona 类。
  前一版 leek 的 4-persona 是 Rust 写死的代码路径——那才是要删
  的对象，不是"多个 subagent"本身。具体怎么配置 subagent 还在
  调研（CC vs codex 的做法对比），见 §4.2。
- **不抽象 LLM provider。** 今天访问模型只有一条路径：codex pro
  via OAuth。代码直接连这条路径。等真有第二个具体 provider 带着
  真实契约出现时再抽象——不要提前。
- **vault 里不把 charters / decisions / portfolio / holdings
  做成一等实体**（至少在 M0–M1 阶段）。用户 mandate 就是一段
  注入 system prompt 的文本。等哪个 milestone 真需要时再升级
  为结构化字段。

这些决定都不是随便做的。每一项都试过、观察到延迟/bug 面/认知
负担超过其价值，然后被移除。

---

## 3. 系统形状

```
+-----------------------+        +-----------------------+
|  Frontend (Solid)     | <----> | Gateway (Rust)        |
|  - chat 面板          |  SSE   | - HTTP + SSE          |
|  - corpus 浏览        |  HTTP  | - auth (OAuth)        |
|  - plan / canvas      |        | - vault (SQLite)      |
|                       |        | - main agent loop     |
+-----------------------+        | - subagent spawn      |
                                 | - tool registry       |
                                 | - corpus loader       |
                                 | - skill / hook engine |
                                 +-----------------------+
                                            |
                                            v
                                 +-----------------------+
                                 | codex pro (OAuth)     |
                                 | Responses API         |
                                 +-----------------------+
```

- **单 Rust crate**（`crates/gateway`）承载所有后端。除非有真实
  理由（独立 CLI、抽出 SDK 之类），不要拆 crate。
- **单一 LLM 访问路径**：codex OAuth → Responses API。不做
  `LlmProvider` trait，不做 `OpenAIClient` / `AnthropicClient`
  这种并行类层级。就一个具体 client。
- **单 vault**：每个用户一份 SQLite 文件。M0 表：`users`、
  `sessions`、`messages`、`user_settings`。其它表只能通过明确的
  milestone 引入，迁移文件要在 commit 时被审查。
- **SSE 做流式**：模型输出、工具事件、计划更新走同一条 SSE 通道。

---

## 4. Agent 拓扑

### 4.1 主 agent

跑在用户的 session 里，每个活跃 session 一个。

System prompt 组装顺序（每节都可选）。**顺序原则：越普适、越稳定
的内容越靠前**——这样 KV cache 的 prefix 能命中得更深，跨 session、
跨用户都能复用。从最稳到最易变：

1. **身份描述（identity）**——leek 自己的短描述。所有用户、所有
   session 都一样。最稳定，放最前。
2. **Corpus orientation**——少量从 corpus 里挑出的"始终适用"的
   片段（原则、定义）。corpus 更新时才会变，频率低。长文走工具
   按需加载，不进 prompt。
3. **可用工具列表**——名字 + 第一行描述，从工具注册表取。仅作
   定位信息，模型实际选工具是从 API `tools` 数组拿，不是从这个
   列表。工具集变化时才变。
4. **Skill 索引**——每个 skill 一行：`name — frontmatter
   description`。body 通过 `use_skill` 懒加载。索引来源三层：内置
   `harness/skills/` + 用户全局 `~/.leek/skills/` + 项目级
   `<project>/.leek/skills/`（第三方 skill 通过界面安装到用户全局
   路径）。skill 集合变化时才变。
5. **用户 mandate**——用户的投资 profile。每个用户不一样、且
   用户编辑后会变，所以放最后；前面 1–4 的 cache 命中不受 mandate
   差异影响。

明确**不进** system prompt 的内容：

- 不写"什么时候用哪个工具"的说明文。工具的选择依据是注册表里每个
  工具自己的 description。
- 不写 deliverable 框架。模型产出什么由用户提问决定。
- 不写"必须先建计划"的强制。`update_plan` 作为一个工具提供，
  模型觉得有用就调用。

### 4.2 Subagent

由主 agent（或另一个 subagent，最多 depth=2）通过 `task` 工具
spawn——沿用 CC 的命名约定。

抽象层定义（**这层是 locked 的**）：每个 subagent 拿到自己的
system prompt + 自己的工具子集 + 自己的 context window + 自己的
loop 实例（跑同样的 loop 代码——同样的安全网，同样的 metrics）。
loop 代码是统一的，**专业化只来自 system prompt + 工具子集**——
没有第二份"subagent loop 实现"。

具体怎么*提供* system prompt + 工具子集（**调研后 lock**）：
照搬 CC——subagent 是磁盘上的 markdown 文件，**frontmatter 定义
配置 + body 是 system prompt 原文**。详细文件格式见下面的 4.2.1。

**关键：与 skill 严格分离。** skill 是注入主 agent system prompt
索引的"上下文展开式"内容（每个 skill 一行 description 进索引，
body 通过 `use_skill` 懒加载到当前 loop）；agent 是被 `task()`
spawn 出去带独立 loop 的子 agent，主 agent 不会把它当 skill 索引、
不能 `use_skill` 调用。两者目录分开（`harness/skills/` vs
`harness/agents/`）、文件名不同（`SKILL.md` vs `AGENT.md`）、
frontmatter 里 description 的写法也不一样：

- skill 的 description 是"该 skill 包含什么知识/指南"——决定要不要
  expand 到当前上下文
- agent 的 description 是"该 agent 能做什么委派工作"——决定要不要
  spawn 一个子 loop

如果混在一起，subagent 会被主 agent 当成 skill 引用，造成调用方式
混乱。两者必须分开维护。

#### 4.2.1 AGENT.md 文件格式

```yaml
---
description: <一句话描述这个 agent 能做什么委派工作>
allowed_tools: [...]      # 可选；不给等于全工具集
model: <override>         # 可选；codex OAuth 单 model 时无视
---

<markdown body — 直接作为 subagent 的 system prompt>
```

发现路径（三层，越靠后优先级越高）：

- 内置（leek 自带，跟随 repo 走）：`harness/agents/<name>/AGENT.md`
- 用户全局：`~/.leek/agents/<name>/AGENT.md`
- 项目级：`<project>/.leek/agents/<name>/AGENT.md`

同名冲突时优先级"项目 > 用户 > 内置"。所有层都进 agent 注册表。
第三方 agent 通过界面安装到用户全局路径。

通信方式**一次性**：父级通过 `task(agent_name, input)` 工具传入
prompt，subagent 返回一个 text block。v0 不做流式回传（之后觉得
有用再加）。

#### 4.2.2 leek 自带的初始 subagent（rebuild-clean 阶段三个）

每个是一个 AGENT.md 文件，跟随 repo 走：

- **`corpus-expert`**——`harness/agents/corpus-expert/AGENT.md`。
  body 是"你深谙 corpus，用户问一个 corpus 相关的问题，给出有原文
  引证的综合回答"。`allowed_tools`：`corpus_search`、`corpus_read`。
  对标 CC 的 `claude-code-guide`。
- **`market-data-fetcher`**——`harness/agents/market-data-fetcher/AGENT.md`。
  对一组 ticker 并行查询。`allowed_tools`：market / fundamentals
  类工具。
- **`planner`**——`harness/agents/planner/AGENT.md`。多步研究的任务
  分解。`allowed_tools`：最小（不做数据查询）——只产出 plan，
  不执行。

### 4.3 为什么投资领域天然适合 subagent

- **Context window 节约**——"NVDA 深度复盘"可能要从 corpus 拉 20
  份文档、6 个 quote 快照、4 个季度的财报。全塞 main loop 会爆
  context。subagent 干重活、返回 digest，父级 context 保持干净。
- **并行能力**——多 ticker 扫描（"对比 BABA / 9988.HK / PDD / JD"）
  天然能拆成 N 个并行 subagent 调用。
- **不靠 persona 动物园做专业化**——`corpus-expert` 就是 prompt
  + 工具子集，不是硬编码代码路径。增加 `crypto-research-expert`
  或 `event-driven-trader` 是改 markdown，不是改 Rust。

---

## 5. Harness 原语

这些是任何现代 agent harness 都有的东西。我们整套照搬；之前
`rebuild` 分支落地的 M1 工作为实现提供参考，但在 rebuild-clean
更干净的底盘上重做一遍。

| 原语                | 默认值                  | 作用域           | 备注                                                                                  |
|---------------------|-------------------------|-------------------|---------------------------------------------------------------------------------------|
| Idle timeout        | 90 秒，默认开           | per-stream        | 对齐 CC 的 `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`。主要的响应性保护。                  |
| Wall-clock 上限     | 30 分钟，默认开         | per-turn          | 硬取消。剩 10/5/2/1 分钟时插软提示（leek 自创）。                                     |
| Iteration cap       | None，opt-in            | per-turn          | codex / CC 都不强制。给高级用户用；默认不开。                                         |
| Cost cap            | None，opt-in            | per-turn          | 用 per-model 价格表算的美元上限。                                                     |
| Doom-loop detector  | N=3，默认开             | per-turn          | 同样的 `(tool_name, args)` 连续 ≥ N 次 → 中止。                                       |
| Auto-compaction     | 90%，默认开             | per-session       | 对齐 codex。                                                                          |
| Per-turn metrics    | 默认开                  | per-turn          | 每个 turn 一行：stop_reason、tokens、cost、first_triggered_guard、iteration。         |

Subagent 的 loop **全部**复用这些。一个挂掉的 subagent 不能污染
父级 UX——保护是统一的。

命名提醒：*turn* = 一次用户 prompt → 一次最终 assistant 回应；
*iteration* = 一个 turn 内的一次 LLM 调用。metrics 表按 turn
而不是 iteration 记录。

---

## 6. 存储（vault）

每用户一份 SQLite。M0 schema：

```sql
users(id, oauth_subject, created_at)
sessions(id, user_id, title, created_at, last_active_at)
messages(seq, session_id, user_id, role, content, created_at, ...)
user_settings(user_id, ...)
turn_metrics(turn_id, session_id, ...)   -- M1 引入
```

M0 故意不引入的表：
`tasks`、`deliverables`、`charters`、`decisions`、`plans`、
`holdings`、`provider_configs`、`compactions`、`tool_runs`、
`subagents`（subagent spawn 不需要单独的表——`turn_metrics`
带 `parent_turn_id` 字段就够）。

每张新加入的表都必须在 migration 文件头部说明**为什么现在加**。
没写出理由的 migration 在 code review 阶段就该被拒。

---

## 7. 用户 mandate

"用户想从这个 agent 那里得到什么"的投资域版本。它能涵盖比如：

- **持仓**（ticker、规模、成本）
- **风险偏好**（明确陈述的）
- **风格偏好**（成长 / 价值 / GARP / 事件驱动 / 等）
- **时间维度**（日内 / 持仓 / 长期）
- **实际交易的市场和币种**
- **硬约束**（"不做杠杆产品"、"不做加密"等）

计划怎么处理（细节待 refine，见 Open Questions）：

- **持久化**：`user_settings.mandate_text`——一段 markdown，
  用户可编辑，在 vault 里有版本记录。
- **注入**：原文写进主 agent 的 system prompt（§4.1 第 5 节，
  最靠后以避免污染前面 1–4 的 KV cache prefix）。
- **收集**：通过 onboarding skill（`harness/skills/mandate/`）
  ——首次 session 时跑，问 4–6 个问题，把结果写下来。之后可以
  在 settings 里编辑。
- **subagent 可见性**：subagent **不**自动继承 mandate。如果某个
  subagent 需要，父级在 task prompt 里传相关切片。

**这部分是整个设计里我们最不确定的部分**（见 §11）。

---

## 8. Corpus

两个接触面：

1. **默认注入**到主 agent 的 system prompt。
   - 一小撮精选的 corpus 片段——原则、定义、反复出现的框架。
   - 有上限（目标：< 800 tokens）。长文通过工具拿。
2. **工具**用于按需检索。
   - `corpus_search(query) → [hit{id, title, snippet}]`
   - `corpus_read(id) → full body`
   - `corpus-expert` subagent 用于需要跨多份 corpus 综合的问题型查询。

存储：markdown 文件放在一个 corpus 根目录下。先用 git 版本管理；
后续 corpus 编辑流程也许会走专门的内容管理面（M2 open question）。

检索：v0 先做 lexical（BM25）。如果召回不够再上 embedding。

---

## 9. Skill / Hook / Plugin

照搬 CC 的约定，做最小调整。**Agent 跟 skill 并行但分开**（见 §4.2
为什么）；下面只说 skill，agent 的对等结构在 §4.2.1。

**Skill**（`<dir>/<name>/SKILL.md`）：

- Frontmatter：`name`、`description`、可选 `allowed_tools`、`model`
- Body：自由 markdown，通过 `use_skill(name)` 懒加载
- 发现路径（三层，越靠后优先级越高）：
  - 内置（leek 自带，跟随 repo 走）：`harness/skills/`
  - 用户全局（第三方 skill 通过界面安装到这里）：`~/.leek/skills/`
  - 项目级（项目 repo 自带的 skill）：`<project>/.leek/skills/`
- 三层都进 skill 索引（写进主 agent 的 system prompt §4.1 第 4 节）。
  同名冲突优先级"项目 > 用户 > 内置"。
- 通过 `notify` 监听做热加载

**Hook** 事件（对齐 CC）：

`PreToolUse`、`PostToolUse`、`Stop`、`SubagentStop`、
`SessionStart`、`SessionEnd`、`UserPromptSubmit`、`PreCompact`、
`Notification`

Hook 执行：shell 命令，捕获 stdout / exit code，可配置超时。

**Plugin**：skill + hook + manifest 的 bundle。v0 只支持本地安装。
远程/集市后续再说。

---

## 10. 老 `rebuild` 分支上什么留什么删

这是清理 commit 要用的字面映射表。"留下"的门槛：MVP 直接需要它。
其它一概砍。

### 留下（可能会有重构）

- `crates/gateway/src/main.rs`——入口
- `crates/gateway/src/auth/`——OAuth 登录
- `crates/gateway/src/llm/codex_oauth.rs`——OAuth 流
- `crates/gateway/src/llm/openai_responses.rs`——Responses API client
- `crates/gateway/src/llm/pricing.rs`——per-model 价格表（cost cap 用）
- `crates/gateway/src/events/`——SSE 基础设施
- `crates/gateway/src/api/sessions.rs`——session CRUD（会简化）
- `crates/gateway/src/api/messages.rs`——message POST + stream
- `crates/gateway/src/api/static_files.rs`——前端静态文件
- `crates/gateway/src/api/health.rs`——健康检查
- `crates/gateway/src/vault/mod.rs`、`sessions.rs`、`messages.rs`、
  `user_settings.rs`
- `crates/gateway/src/corpus/`——corpus loader（会简化）
- `crates/gateway/migrations/0001_initial.sql`——user / session /
  message schema
- `crates/gateway/migrations/0007_user_settings.sql`——user settings
- `frontend/web/`——整个前端目录，API 契约会调整
- `harness/skills/`——自带 skill（agent 重建过程中重写 body）
- `harness/identity.md`、`harness/discipline.md`、
  `harness/corpus_orientation.md`——system prompt 片段
- `corpus/`——corpus 内容（目前是空占位）

### 删（DELETE）

- `crates/gateway/src/agent/mod.rs`（2276 行）——重写，做小
- `crates/gateway/src/agent/harness.rs`——system prompt 构造重写
- `crates/gateway/src/agent/routing.rs`——routing 层，整个删除
- `crates/gateway/src/agent/compact.rs`——auto-compact，M1 时折进
  新 loop
- `crates/gateway/src/agent/tools/`——保留*工具定义*但 registry
  plumbing 重做
- `crates/gateway/src/api/charter.rs`
- `crates/gateway/src/api/corpus.rs`（如不再用专门 API 就删）
- `crates/gateway/src/api/deliverables.rs`
- `crates/gateway/src/api/portfolio.rs`
- `crates/gateway/src/api/tools.rs`（legacy）
- `crates/gateway/src/api/stream.rs`（与 events/ 冗余则删）
- `crates/gateway/src/vault/charters.rs`
- `crates/gateway/src/vault/compactions.rs`
- `crates/gateway/src/vault/decisions.rs`
- `crates/gateway/src/vault/holdings.rs`
- `crates/gateway/src/vault/plans.rs`
- `crates/gateway/src/vault/provider_configs.rs`
- `crates/gateway/src/vault/subagents.rs`
- `crates/gateway/src/vault/task_metrics.rs`——M1 时重命名为
  `turn_metrics.rs`
- `crates/gateway/src/vault/tasks.rs`
- `crates/gateway/src/vault/tool_runs.rs`
- `crates/gateway/migrations/0002_compaction.sql`
- `crates/gateway/migrations/0003_decisions.sql`
- `crates/gateway/migrations/0004_charters.sql`
- `crates/gateway/migrations/0005_in_place_compaction.sql`
- `crates/gateway/migrations/0006_agent_plan_items.sql`
- `crates/gateway/migrations/0008_decision_structure.sql`
- `crates/gateway/migrations/0009_plan_resolution.sql`
- `crates/gateway/migrations/0010_task_metrics.sql`——M1 时重命名 + 重塑

### 前端 reshape（M0）

- 删掉绑定到已删除后端实体的卡片/面板（charter 面板、decision
  draft 卡片、portfolio 面板、deliverable artifact、task 状态指
  示器）。
- 保留：chat composer、消息列表、SSE 流接线、corpus 浏览、plan
  视图。
- API 契约对齐到新 gateway。细节在 M0 里定。

---

## 11. 待定的 Open Questions

不会阻塞重建启动。先记下来，等对应 milestone 启动时再敲定。

### 用户 mandate（M2）
- **Onboarding UX**：skill 驱动问答 vs settings 表单 vs 两者都做？
- **可变性**：聊天里改（`/edit-mandate`）vs settings 页面改 vs
  两个都给？
- **mandate 长度上限**：长到 2K tokens 会吃掉每个 system prompt。
  硬限长？还是 save 时跑一遍 summarization？
- **subagent 的 mandate 可见性**：subagent 什么时候看到 mandate？
  始终？skill 显式 opt-in？主 agent 按 task 决定？

### Corpus（M2）
- Embedding vs lexical 检索——lexical 跑 v0，但召回降到什么水平
  就该上 embedding？
- corpus 版本管理 UX——GitOps（每次改提 PR）？应用内编辑器？
  两个都做？

### Subagent（M2.7）
- ~~工具名字~~：locked 为 `task`（对齐 CC）。
- ~~配置机制~~：locked 为 AGENT.md（frontmatter + body，与 skill
  分开维护），见 §4.2.1。
- 事件流：v0 一次性结果给父级 vs 实时流式。先一次性；什么时候
  流式才值得复杂度？
- subagent 在 vault 里怎么算：在父级 session 下开自己的 turn 行
  （父级 turn 的 child），还是单独的 session？默认前者，但跨 turn
  的可观测性 UI 怎么做还没定。
- 自定义 agent 的热加载：复用 `notify` 监听 `~/.leek/agents/` 和
  `<project>/.leek/agents/`，启动延后到第一次有用户写自定义 agent
  时再做。

### Skill / Hook / Plugin（M2.5）
- Plugin 沙箱——v0 punt 到"只信本地安装"，第一次有人想要远程
  plugin 时再回头看。
- Skill 的 model override 跟 codex OAuth 组合时怎么解（CC 每个
  skill 可换 model；codex pro 就一个 model）。

### Codex OAuth 细节
- Token refresh 异常 edge case——refresh 在流式中失败怎么办？
  retry 的边界在哪？
- Rate limit 可观测性——codex pro 限流时，怎么暴露给开发者但
  又不在用户面前露出 provider 身份（违反原则 1）？

---

## 12. 横向原则（所有 milestone 都遵守）

延续自 `rebuild` MILESTONES.md §"Cross-cutting principles"。
这些是**强制**。它们之所以挺过重置，是因为它们关于*怎么写*，
不是关于*写什么*。

1. **工具命名中立。**工具名字、描述、参数文档、错误消息：对厂商
   中立。具体上游身份（Tushare、SEC EDGAR、Yahoo Finance、Binance
   等）只能出现在代码注释、struct 字段、env var 名字、`tracing`
   日志里——永远不出现在模型能读到的任何东西里。

2. **Skill 渐进披露。**System prompt 只列每个 skill 的 frontmatter
   `description`。body 通过 `use_skill(name)` 懒加载。不要从
   description 反推 body——让模型自己加载。

3. **约束 loop 之前先信任 provider。**拿不准时对齐 codex 默认值。
   硬上限误伤太多正常场景（claude-code 删掉的 5 分钟超时是个
   经典教训）。能力 opt-in；可观测性默认开。

4. **工程决策归 agent，产品决策归用户。**实现层选择委托给 agent。
   产品形态、UX 形态、功能优先级——用户拍板。

5. **工程决策要明面化。**做了用户可能想反悔的工程决策时，写在
   回复里——别埋在事后 commit message 里。

6. **窄优先于深。**M1 是宽而浅（loop 基础设施）。M3 是窄而深
   （先 A 股做好）。当前 vertical 没证明自己之前不要横向扩张。

7. **（新加）不要把确定性系统思维带进 agent。**这是上一次 rebuild
   失败的代价。Web app 工作流有状态机和校验关，是因为用户按已知
   顺序点按钮。Agent 的下一步是模型决定的。Routing 层、deliverable
   分类、拦截模型的 plan guard——所有这些都试图给 agent **强加**
   确定性，结果产出的是 bug 面。Agent harness 就是一个 loop 加一
   个工具列表。其它任何东西都要过高门槛：codex 或 CC 有这玩意吗？
   没有的话，leek 凭什么要？
