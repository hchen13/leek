# L.E.E.K 架构设计

> **rebuild-clean 分支的端态规格说明。**
>
> 配套文档：`docs/REQUIREMENTS.md` 与 `docs/MILESTONES.md`。
> REQUIREMENTS 告诉你产品 / UX / 验收边界；MILESTONES 告诉你
> *什么时候发布什么*；本文档告诉你*全部里程碑落地后系统长什么样*。
> 三份保持同步——本文档里某个架构决策有变动时，找到对应的需求和
> milestone 一起更新。
>
> 最近修订：2026-05-19。

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

除此之外都跟业内已经收敛的 agent harness 模式一致。我们不发明
agent loop 原语。我们采纳它们。

> **关于 memory / 用户 mandate**：harness 里的"用户 mandate"
> （持仓 / 风格 / 风险偏好 / 跨 session 持久化的语义状态）本质上
> 是 memory 的一个特例。memory 是一个**独立的设计课题**——分层
> （项目级 / 用户级 / session 级）、编辑 UX、冲突解决，CC / codex
> 的最佳实践还在演进中。M0–M4 不做这一层，等 leek 跑起来、看到
> 真实使用模式之后再单独研究。详见决策日志 2026-05-11。

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
  agent（`corpus-expert`、未来的领域取数 subagent 等）是想要的；
  但它们的 system prompt 和工具子集通过**外部配置**（AGENT.md
  文件）提供，而不是 Rust 里的 hardcoded persona 类。前一版 leek
  的 4-persona 是 Rust 写死的代码路径——那才是要删的对象，不是
  "多个 subagent"本身。配置机制见 §4.2。
- **不抽象 LLM provider。** 今天访问模型只有一条路径：codex pro
  via OAuth。代码直接连这条路径。等真有第二个具体 provider 带着
  真实契约出现时再抽象——不要提前。
- **vault 里不存任何用户特定的语义数据**（charters / decisions /
  portfolio / holdings / mandate / memory 等等）。这些归到独立的
  memory 课题里，M0–M4 不碰。

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
  `sessions`、`messages`、`events`。其它表只能通过明确的 milestone
  引入，迁移文件要在 commit 时被审查。
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

未来加 memory / 用户 mandate 层时会插在第 4 之后（最易变、最不
通用），不影响前面 1–4 的 cache prefix。

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

#### 4.2.2 内置 subagent

leek 自带的 subagent 分两类，每个都是一个 AGENT.md 文件，跟随
repo 走。

**`general-purpose`（基线——架构上必须有）**

- `harness/agents/general-purpose/AGENT.md`
- system prompt 通用："你是被委派的 worker，完成给定的子任务，
  把结果汇总成一个 text block 返回"
- `allowed_tools`：全集
- 用途：主 agent 把一段自包含的多步工作委派出去——subagent 在
  自己的 context window 里跑完、返回 digest，主 agent 的 context
  保持干净。`task()` 不指定 agent 时默认就是它。
- 它是 subagent 机制的"无专业化"基线形态：其它专业 subagent
  本质上就是 general-purpose + 受限 system prompt + 工具子集。

**专业化 subagent（随依赖落地逐个加）**

- **`corpus-expert`**——`harness/agents/corpus-expert/AGENT.md`。
  body 是"你深谙 corpus，用户问一个 corpus 相关的问题，给出有原文
  引证的综合回答"。`allowed_tools`：`corpus_search`、`corpus_read`。
  对标 CC 的 `claude-code-guide`。依赖 corpus 工具（M2），M2.7
  起可用。
- 领域 subagent（例：并行查行情/财务的取数 subagent）——依赖
  M3 的领域工具。具体有哪些、各自的工具子集，等 M3 规划领域工具
  时一起定，本文档不提前写死。

**不做 `planner` subagent。** 计划是主 agent 用 `update_plan` 工具
就地做的事，不是委派出去的事。单独 spawn 一个"只产出 plan 不执行"
的 subagent 只是多一个来回 + 一次 context 交接，没有收益。如果
做计划本身需要先调研，那部分调研可以委派给 general-purpose /
corpus-expert，主 agent 再据此 `update_plan`。

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

| 原语                       | 默认值                  | 作用域           | 备注                                                                                  |
|----------------------------|-------------------------|-------------------|---------------------------------------------------------------------------------------|
| Idle timeout               | **180 秒，默认开** (M3.6) | per-stream        | 原 90 秒（对齐 CC `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`），但 xhigh reasoning + 20+ tool turn 经常超 90s silence 把长 prompt 强 fatal。M3.6 加倍到 180s。 |
| Wall-clock 上限            | 30 分钟，默认开         | per-turn          | 硬取消。剩 10/5/2/1 分钟时插软提示（leek 自创）。                                     |
| Iteration cap              | None，opt-in            | per-turn          | codex / CC 都不强制。给高级用户用；默认不开。                                         |
| Cost cap                   | None，opt-in            | per-turn          | 用 per-model 价格表算的美元上限。**M3.6: subagent 可在 AGENT.md frontmatter `cost_cap_usd` 字段独立覆盖**——只对该 subagent loop 生效。 |
| Doom-loop detector         | N=3，默认开             | per-turn          | 同样的 `(tool_name, args)` 连续 ≥ N 次 → 中止。                                       |
| Auto-compaction            | 90%，默认开             | per-turn/session  | 到阈值时摘要压缩旧上下文并继续。这是上下文接近上限时的唯一设计行为；不做停 turn 的护栏。 |
| Codex builtin URL warn     | N=3，默认开             | per-turn          | 同一 `(action, url)` ≥ N 次 → canvas warn + next-iter hint。                          |
| Codex builtin URL abort    | **0，默认关闭** (M3.6)  | per-turn          | 原 N=7 自动 abort，但 deep-dive 多角度重读权威源是合法行为，强杀让用户拿不到答案。M3.6 默认关掉，只 warn；用户仍可 PATCH `builtin_url_abort_threshold` 重新启用。 |
| Provider retry             | 5 次（1 + 4 retry），默认开 | per-codex-call   | M3.5：1s/5s/15s/30s 指数 backoff，5xx / connection_failed / 流中 timeout 自动重试，每次 retry 前 emit `provider_retry_attempt` lifecycle event。 |
| Per-turn metrics           | 默认开                  | per-turn          | 每个 turn 一行：stop_reason、tokens、cost、first_triggered_guard、iteration。         |

Subagent 的 loop **全部**复用这些。一个挂掉的 subagent 不能污染
父级 UX——保护是统一的。

命名提醒：*turn* = 一次用户 prompt → 一次最终 assistant 回应；
*iteration* = 一个 turn 内的一次 LLM 调用。metrics 表按 turn
而不是 iteration 记录。

**Context window 与 compaction 阈值都可配置，默认对齐 codex。**
gpt-5.5 经 codex 后端的 raw context window 是 **272K**（codex
`/models` 的实际值，见 decision log 2026-05-19；leek 早先硬编码的
400K 是错的猜测）。auto-compaction 触发点 = context window × 阈值
（默认 0.90）= 244.8K，与 codex 一致。两个量都能用 env 覆盖——
`LEEK_CONTEXT_WINDOW` 和 `LEEK_AUTO_COMPACT_THRESHOLD`（测试 compaction
时把窗口设小，几个 tool-heavy turn 就能触发）。leek **不**采用 codex
的 `effective_context_window_percent`（95%）——90% 的 compaction 比那条
可用上限更早触发，95% 不会成为约束。codex 对 gpt-5.5 的 `max_context_window`
也是 272K，所以窗口 override 实际只能往下调（往上后端会拒）。

---

## 6. 存储（vault）

每用户一份 SQLite。M0 schema：

```sql
users(id, created_at)
sessions(id, user_id, title, created_at, last_active_at)
messages(session_id, seq, role, content, created_at)
events(session_id, seq, kind, payload, created_at)
turn_metrics(turn_id, session_id, ...)   -- M1 引入
```

M0 是单用户本地骨架：`messages` / `events` 不重复存 `user_id`，
归属从 `sessions.user_id` 派生。OAuth 相关字段等 M1 接入 codex
OAuth 时随 token 存储一起引入。

M0 故意不引入的表：
`tasks`、`deliverables`、`charters`、`decisions`、`plans`、
`holdings`、`user_settings`、`provider_configs`、`compactions`、
`tool_runs`、`subagents`（subagent spawn 不需要单独的表——
`turn_metrics` 带 `parent_turn_id` 字段就够）。

每张新加入的表都必须在 migration 文件头部说明**为什么现在加**。
没写出理由的 migration 在 code review 阶段就该被拒。

---

## 7. 用户 mandate / memory（已移除，推迟）

原本这一节设计了一个简陋的 mandate 注入机制（`user_settings.mandate_text`
markdown blob，原文进 system prompt，onboarding skill 收集）。
**2026-05-11 整节移除。**

理由：在 harness 里，"用户 mandate"本质上是 **memory 的一个特例**
——持仓 / 风格 / 风险偏好 / 跨 session 持久化的语义状态。memory
是一个**独立的设计课题**：

- 要做分层（项目级 / 用户级 / session 级，类似 CC 的 CLAUDE.md
  + project memory + user memory）
- 要有编辑 UX、冲突解决、版本管理
- CC / codex 的最佳实践都还在演进中

在 leek 还没跑起来、没有真实使用数据的情况下硬塞一个 mandate 实现，
大概率又是一个 deterministic-systems 包袱（"这个用户用 leek 时
是 X 风格，所以一定要这样回答"——把模型应该自由决定的事情写死）。

**什么时候回来**：M3 A 股 MVP 跑起来之后，看实际研究 session 里
出现什么用户特定的痛点，再回到这个课题——届时调研当时的 CC / codex
memory 实现作为参考。可能成为 leek 的一个独立 milestone（暂称
"M5 — memory"）。

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

## 10. Clean-room rebuild 边界

M0 不再做"旧实现里哪些文件留下来改"的清理题，而是做 clean-room
runtime。原因：partial-retain 会把旧 schema、旧事件、旧 endpoint、
旧 UI 和旧 mental model 一起带进新系统，制造"能编译但方向已偏"
的假象。

### Active tree 保留

只保留不会参与 runtime 控制流的设计和内容资产：

- `AGENTS.md`
- `docs/REQUIREMENTS.md`、`docs/ARCHITECTURE.md`、`docs/MILESTONES.md`
- `design/` 整体只作为历史参考保留（**不是**当前权威——参见
  `REQUIREMENTS.md §0`；locked 决策的现行记录在 `MILESTONES.md`
  decision log）
- `harness/identity.md`、`harness/discipline.md`、
  `harness/corpus_orientation.md`
- `harness/skills/`
- `corpus/`
- workspace / package manifest 中 M0 真正需要的最小部分

### Active tree 清底

这些路径不作为 M0 的继承基础；需要时从 git history 精确查阅：

- `crates/gateway/src/`
- `crates/gateway/migrations/`
- `frontend/web/src/`
- `README.md` 里的旧 quickstart / roadmap 叙述

M0 可以重新创建同名目录和文件，但内容必须按新 milestone 从零写。
禁止把旧 runtime 整目录搬进 `tmp/legacy` 或其它可被误读为状态源的
位置。参考旧代码时使用：

```bash
git show <old-rev>:<path>
git grep <symbol> <old-rev>
```

摘回来的片段必须通过当前架构重审。比如 M1 需要 codex OAuth 时，
可以查旧 `codex_oauth.rs` 的 token refresh 细节，但不能顺手带回
`LlmProvider` trait、routing surface、compaction surface 或
`provider_configs` M0 schema。

### M0 前端边界

M0 默认无产品前端。若为了验收要浏览器端验证，只允许一个极简 chat
harness：

- session list / create
- message list
- message composer
- SSE event log

不能带 portfolio、decision draft、plan、canvas fixture、corpus
browser、settings、compaction、tool cards。那些属于后续 milestone，
从旧前端摘取时也要逐组件重审。

---

## 11. 待定的 Open Questions

不会阻塞重建启动。先记下来，等对应 milestone 启动时再敲定。

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
