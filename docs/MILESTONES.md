# L.E.E.K 重建路线图

> **rebuild-clean 分支的目标和顺序的源头依据。**
>
> 配套文档：`docs/ARCHITECTURE.md`。ARCHITECTURE 告诉你端态形状；
> 本文档告诉你到达端态的顺序。
>
> 每完成一个 milestone 就更新本文档。在 M4 落地之前，产生这些
> milestone 的对话上下文会被压缩很多次——这份文件存在的意义就
> 是让 context 被压缩之后，无论是人还是接力的 LLM 都能不丢决策
> 地重新对焦。

---

## 怎么读这份文件

- 每个 milestone 有 **Scope**、**Sub-commits**（计划中）、**Design
  decisions（locked）**、**Open questions**。
- "Locked" 的决策来自跨仓库调研（codex-rs、claude-code、
  hermes-agent、openclaw）或者明确的用户/agent 对话。不要悄悄
  反悔——想推翻就明面提。
- "Open questions" 是已延后但有跟踪的。在对应 sub-commit 之前
  必须敲定；不要让它烂掉。
- 横向原则放在 `ARCHITECTURE.md §12`。
- 文末的 Decision log 记录的是 locked 决策*为什么*被 lock。

---

## 状态标识（2026-05-11）

老的 `rebuild` 分支完成了 M1 + Phase 0g 清理，之后被诊断为携带
了过多的确定性系统脚手架（routing 层、deliverable 分类、plan_guard），
在预算内无法挽救。**决定：删掉 agent 后端，在 `rebuild-clean` 上
重启。** 前端代码保留。Locked 设计决策保留（它们是研究背书过的，
独立于实现它们的那些差劲代码）。详见 "Decision log" → 2026-05-11 条目。

老的 milestone 完成标记（M1 DONE 等）有意被重置——*实现*它们的
*代码*正在被删除。它们*验证过的原则*仍然 locked。

---

## M0 — Clean skeleton（干净骨架）

### 目标

立起最小端到端可用的纵切片，用来证明 plumbing 通了：用户能登录、
开 session、发消息、通过 SSE 收到响应。**这个阶段没有 agent loop。
没有工具。没有 LLM 调用。** "响应"是服务端 echo 回来。我们要证明
的是前后端接通。

### Scope

- 分支手术：删除老的 agent / routing / deliverable / charter /
  decision / plan / portfolio / holdings / subagents / task_metrics
  / tool_runs / compaction 代码路径。字面清单见 `ARCHITECTURE.md
  §10`。
- Migration 合并：把老的 10 份 migration 合成一份新的
  `0001_initial.sql`，覆盖 M0 的 schema（`users`、`sessions`、
  `messages`、`user_settings`）。
- 前端 reshape：删掉绑定到已删除后端实体的卡片/面板（charter、
  decision draft、portfolio、deliverable artifact、task 状态）。
  保留 chat composer、消息列表、SSE 接线、corpus 浏览、plan 视图
  （plan 视图 M1 之前先休眠，这没问题）。
- 验证：发一条消息，看它持久化，看到 SSE echo 响应回来。没有
  agent，没有 LLM。

### Sub-commits（计划）

| #    | 标题               | Scope                                                                                              |
|------|--------------------|----------------------------------------------------------------------------------------------------|
| M0.1 | 删除老后端          | 移除全部 `crates/gateway/src/agent/`、被删除的 vault 模块、被删除的 API 路由                         |
| M0.2 | Migration 重置      | 把 migrations 0001–0010 合并成一份新的 `0001_initial.sql`                                            |
| M0.3 | Echo loop          | 服务端桩：POST 消息 → SSE 事件 echo 响应 → DB 持久化                                                 |
| M0.4 | 前端 reshape       | 移除已删除实体相关的面板；验证端到端 echo 通                                                          |

### Design decisions（locked）

- **M0 用单一 migration 文件**，不做历史保留。我们不保留老 vault
  格式。新用户用新 schema。
- **M0 用 echo，不调 LLM**。把 plumbing 和 agent 逻辑分开能让 M0
  小、让 M1 端到端地拿到模型集成的所有权。
- **不存 `tasks` 表。**会话就是消息序列。见 `ARCHITECTURE.md §6`。

### Open questions

- 开发机上现存的 vault DB——丢掉还是写个一次性 migrator？默认：
  丢掉（我们还没有生产用户）。

---

## M1 — Agent Loop MVP：安全网

### 目标

跟老的 M1 一样（已经验证过这个形状是对的）：让主 agent loop
端到端跑通——基于 codex OAuth，所有 harness 安全网都到位。要
设上限，但大多数上限默认 opt-in（对齐 codex 的"信任 provider"哲学）。

### Scope

- 主 agent loop：codex OAuth → Responses API → SSE 流式回给前端
- 工具注册表接线（1–2 个微型工具：`web_fetch`，也许 `echo` 用来
  做测试）
- 安全网（见下面的 locked 决策表）
- `turn_metrics` vault 表 + 写入钩子
- per-turn 可观测性事件（`turn_metrics_recorded` SSE）

### Sub-commits（计划）

| #    | 标题                                                        | 默认值          |
|------|-------------------------------------------------------------|-----------------|
| M1.1 | turn_metrics 表 + GuardConfig 脚手架                        | —               |
| M1.2 | Codex OAuth 调用 + 裸 loop（先没有 guard，就跑迭代）         | —               |
| M1.3 | Idle timeout                                                | 90 秒，默认开    |
| M1.4 | Wall-clock 上限 + 阶段化 soft-prompts                       | 30 分钟，默认开 |
| M1.5 | Iteration cap                                               | None，opt-in    |
| M1.6 | Cost cap + per-model 价格表                                 | None，opt-in    |
| M1.7 | Doom-loop detector + first_triggered_guard 接线             | N=3，默认开      |
| M1.8 | Auto-compaction                                             | 90%，默认开      |

### Design decisions（locked — 从老 rebuild 继承）

下面这些在老 `rebuild` 分支通过跨仓库调研被验证过。验证结果
跟实现它们的代码无关，所以验证保留。

- **Iteration cap opt-in，默认 `None`** — codex 和 claude-code
  的核心 loop 都不强制。codex 明确"靠 auto-compaction"。老 leek
  硬编码 `MAX_TOOL_TURNS=24` 偏紧（真复杂的 A 股研究经常合理地
  跑 20+ 轮）。openclaw 是 `[32, 160]`。我们倒向 codex / CC。

- **Cost cap opt-in，默认 `None`** — codex 不跟踪成本。leek 把
  机制接线给高级用户 / 生产使用，但默认不开。

- **Wall-clock 上限 30 分钟，默认开** — claude-code 历史上有过
  5 分钟硬超时；他们的 CHANGELOG 明确写"作为 bug 移除了"。30
  分钟是真的 edge case 上限，不是日常 guard。

- **Idle timeout 90 秒，默认开** — 对齐 claude-code 的
  `CLAUDE_STREAM_IDLE_TIMEOUT_MS=90000`。主要的响应性保护。
  openclaw 有类似的 `turnCompletionIdleTimeoutMs=60s`。

- **Doom-loop detector N=3，默认开** — leek 自创。codex / CC /
  hermes / openclaw 都没有等价物。同样的 `(tool_name, args)`
  连续 ≥ 3 次时触发。

- **Auto-compaction 90%** — 对齐 codex 的硬编码
  `(context_window * 9) / 10`。

- **Soft-prompt 时间提示是 leek 自创** — 阶段化 10/5/2/1 分钟
  阈值，按 LLM block 注入（不是按 turn）。剩余 > 10 分钟时不
  注入——大多数 turn 看不到这个 guard。

  阶段化文案：
  - `≤ 60s`: "立刻收尾，用现有信息给结论，别再调工具"
  - `61–120s`: "现在写一个简洁的结论；完成已经在跑的工具调用，但不要开新的"
  - `121–300s`: "开始组织最终回答；非关键调查可以延后"
  - `301–600s`: "考虑缩小分析范围；如果还有多个分支，优先广度而非深度"

- **不发 `ChatRequest.max_output_tokens`** — codex-rs 的
  `ResponsesApiRequest` struct 里没这个字段。信任 provider
  的 per-model 默认值。

- **不抽象 LLM provider** — 当前只有一条路径（codex OAuth）。
  等真的第二个具体 provider 出现时再抽象。省了一个 trait 层级
  的维护成本，也避免误导性的"很容易切换 provider"声明。

### 命名约定（locked）

- *turn*：一次用户 prompt → 一次最终 assistant 消息
- *iteration*：一个 turn 内的一次 LLM 调用
- `turn_metrics` 表按 turn 做主键，每 turn 一行
- 内部 loop 变量是 `iteration_count`，不叫 `turn`

### Open questions

- per-model 的 wall-clock 默认值？（目前全局 30 分钟。reasoning
  模型也许要更长。）推到第一次真有人抱怨再说。
- Cost cap 怎么处理多档价格（input vs cached input vs reasoning
  vs output）——具体 schema 在 M1.6 commit message 里定。

---

## M2 — Corpus + Mandate

### 目标

把让 leek 区别于通用 agent 的两块内容搞起来：投资 **corpus** 和
每用户的 **mandate**。两个都通过主 agent 的 system prompt 和工具
对外暴露。

### Scope

- Corpus loader：从一个根目录读 markdown，做 lexical（BM25）检索
- 工具：`corpus_search(query)`、`corpus_read(id)`
- 默认 corpus 注入到主 agent 的 system prompt（目标 < 800 tokens）
- 用户 mandate：`user_settings.mandate_text`，写进 system prompt
- Mandate 收集 UX（onboarding 流） + 编辑 UX（settings 页面或者
  聊天里的 slash 命令——见 Open questions）

### Sub-commits（计划）

| #    | 标题                                                |
|------|-----------------------------------------------------|
| M2.1 | Corpus loader + BM25 索引，内存中                   |
| M2.2 | `corpus_search` + `corpus_read` 工具                |
| M2.3 | 系统 prompt 默认注入精选 corpus 片段                |
| M2.4 | `user_settings.mandate_text` + 原文注入             |
| M2.5 | Mandate 编辑 UX（settings）                         |
| M2.6 | Mandate 收集 onboarding（首次 session 流）          |

### Design decisions（locked）

- **先 lexical，embedding 后说。** embedding 有 setup 成本，corpus
  编辑还要重算。先 punt，等 lexical 召回有可测量的不足再上。
- **Corpus 用 git 版本管理。** 通过编辑 markdown 文件作者化。
  应用内编辑器是后续 affordance。
- **Mandate 是一段 markdown，不是结构化字段。** 我们不知道哪些
  字段重要，先看 LLM 在自由 text 上怎么用，等明显的模式浮出来
  再结构化。

### Open questions

- Mandate 长度上限——长到超过 2K tokens 时怎么办？硬限长还是
  save 时跑 summarization？
- Mandate 编辑 UX surface：聊天里的 slash 命令 vs settings 页面
  vs 两个都给？
- Corpus 更新 reload——v0 启动时加载就够，但什么时候开始想做热
  加载？

---

## M2.5 — Skill / Hook / Plugin

### 目标

把 skill / hook / plugin 做到一等，对齐 Claude Code 成熟的实现。
M2.5 **不**是发明的地方——抄约定，做最小适配。

### Scope

**Skill**：
- 发现：自带（`harness/skills/`）、用户目录（`~/.leek/skills/`）、
  项目目录（`<project>/.leek/skills/`）
- Frontmatter：`name`、`description`、可选 `allowed_tools`、
  `paths`、`disable-model-invocation`、`model`
- `use_skill(name)` 工具懒加载 body
- System prompt 里的 skill 索引（只 description，每个 skill 一行）
- Skill → tool 限制：skill body 内的工具调用受限于该 skill 的
  `allowed_tools`
- 通过 `notify` crate 做热加载

**Hook**：
- 对齐 CC 的事件面：`PreToolUse`、`PostToolUse`、`Stop`、
  `SubagentStop`、`SessionStart`、`SessionEnd`、`UserPromptSubmit`、
  `PreCompact`、`Notification`
- Hook 执行：shell 命令（按 CC 契约捕获 stdout / exit code）
- Hook 超时（CC 有 per-hook `timeout` 字段，5–60 秒常见）
- Block / continue 语义

**Plugin**：
- skill + hook + commands 的 bundle，作为一个单位分发
- Manifest 格式对齐 CC plugin
- v0 只支持本地安装

### Open questions

- Mandate 机制在这个模型下放哪？很可能变成一个默认安装的 skill
  （`harness/skills/mandate/`），加一个 `SessionStart` hook 把
  mandate-text 注入 system prompt。M2.5 commit 时定。
- Plugin 沙箱——第一版不做；只信本地安装。
- Skill 的 model override 跟 codex OAuth 组合时怎么解（CC 每个
  skill 可换 model；codex pro 就一个 model）。

---

## M2.7 — Subagent

### 目标

一个通用机制，用来 spawn 一个有自己 context window、system
prompt、工具子集的子 agent loop。投资领域真的吃这个（多 ticker
并行扫描、corpus-expert 委派、planner 隔离）。topology 的理由
见 `ARCHITECTURE.md §4.2-4.3`。

### Scope

- 一个 `task` 工具（CC 约定）给主 agent 调用
- subagent 在自己的 loop 里跑：自己的 system prompt（通常 skill
  body）、自己的工具子集、自己的消息历史
- **subagent loop 复用所有 M1 guard**（cost cap / wall-clock /
  idle / iteration / doom-loop / turn_metrics）
- 结果作为单个 text block 返回给父级（v0 不流式）
- skill 驱动的 persona 绑定：`task(skill="corpus-expert",
  input="...")` 把 skill body 当成 subagent 的 system prompt，
  工具子集限制为 skill 的 `allowed_tools`
- 嵌套：subagent 可以 spawn subagent，默认 depth 上限 2（主 →
  子 → 孙就停）

### Sub-commits（计划）

| #       | 标题                                                                    |
|---------|-------------------------------------------------------------------------|
| M2.7.1  | `task` 工具 + subagent loop spawn                                       |
| M2.7.2  | Skill 驱动的 persona 绑定                                                |
| M2.7.3  | Depth 上限 + per-subagent turn_metrics 行（parent_turn_id 链接）          |
| M2.7.4  | 前三个 subagent skill：`corpus-expert`、`market-data-fetcher`、`planner` |

### Design decisions（locked）

- **工具名字 `task`** — CC 约定，没理由不同。
- **v0 一次性结果（batch），不流式** — 更简单；UX 真有需求再升级。
- **Subagent 在 vault 里的归属：在父 session 下开自己的 turn** —
  不开独立 session 实体。`turn_metrics.parent_turn_id` 承担链接。
- **默认 depth 上限 2** — 通过 `turn_metrics.depth` 跟踪。

### 依赖

- **依赖 M1** — 安全网也要在 subagent loop 里工作。
- **依赖 M2.5 skill** — 通过 skill 做 persona 绑定是主用例。
- **解锁 M3** — A 股的 task 形态（全市场扫 + 个股深度 review
  并行分支）需要它。

### Open questions

- 事件流：实时流给父级 vs 一次性 batch。默认 batch；第一次有用
  户可感知的延迟抱怨时再回头看。
- Subagent 的 mandate 可见性——见 `ARCHITECTURE.md §7` open
  questions。

---

## M3 — A 股 MVP

### 目标

5–7 个核心 A 股工具 + 3 种 task 形态（快速扫描、深度复盘、对比）。
工具能用、prompt 能用、task 形态能复用。

### Scope（高层——M3 启动时再细化）

工具（从已删除的 `agent/tools/` 集合里重新引入，但要在新的命名
中立和 harness 适配视角下重审）：

- `market_quote` — 快照报价
- `get_candlesticks` — 各市场 OHLCV
- `get_financials` — 利润表 / 资产负债表 / 现金流 / 比率
- `get_company_info` — 公司画像 + 最新指标
- `get_capital_flow` — 资金流向 + 北向（北向日数据不可用时优雅退化）

Task 形态——有 eval case 端到端跑通：

1. **快速扫描** — "X 现在能不能交易 / 值不值得看？"一个 subagent
   （`market-data-fetcher`）取数据，主 agent 综合。< 2 分钟 wall-
   clock。
2. **深度复盘** — 完整个股 review。多个 subagent 并行（数据获取
   + corpus expert + planner）。5–15 分钟 wall-clock 典型。
3. **对比** — N 个 ticker。N 个并行 `market-data-fetcher` subagent，
   主 agent 综合。

### Open questions

- 哪 5–7 个工具是合理的初始集？上面列的只是起点假说。用前 10
  个真实研究问题验证。
- Eval 集——测试 query 从哪里来？很可能来自用户自己的研究历史。

---

## M4 — A 股完整版

### 目标（占位）

生产可用的 A 股研究纵向：所有常见研究问题形态覆盖、跨 session
保留结论、可观测性仪表盘。

[细节范围 M3 落地、看到缺什么之后再细化。]

---

## Decision log（按时间顺序）

### 2026-05-09 — rebuild 分支方向
- 拆除（在已经被删的 `rebuild` 分支上）：critic / 4-persona
  subagent / decision_draft pipeline / budget_finalization。
- 保留：4 个面板（chat / canvas / corpus / plan）。
- 路线：尽量对齐 codex 风格约定。

### 2026-05-09 — 时间的 soft-prompt + 硬上限
- Wall-clock 同时上 soft prompt（剩 10/5/2/1 分钟时按 block 注入）
  和硬上限（30 分钟取消）。
- Soft 是 leek 自创；硬上限保守。
- 跨仓库调研：codex（无）、claude-code（无，作为 bug 移除）、
  hermes-agent（无）、openclaw（只 idle）。

### 2026-05-09 — Guard 的 opt-in vs 默认开
- 默认开：idle timeout、wall-clock、doom-loop、auto-compaction、
  可观测性（`turn_metrics`）。
- Opt-in：iteration cap、cost cap。
- 对齐 codex，除了 leek 自创的 guard（doom-loop、soft 时间提示）
  默认开有意义。
- 跨仓库调研：codex（无 iteration cap、无 cost cap、90%
  auto-compact、无 `max_output_tokens`）、claude-code（无
  iteration cap、无 per-call max_tokens 默认）、openclaw
  （iteration cap [32, 160] 缩放、idle timeout 60s、有 retry
  cap）、hermes-agent（完全无 per-turn cap，只有 HTTP 层）。

### 2026-05-09 — Subagent 加为 M2.7
- 初版 roadmap 漏了。加在 skill/hook/plugin（M2.5）和 A 股 MVP
  （M3）之间，因为 subagent 依赖 skill 机制（persona 绑定），
  也是 A 股并行 task 形态的解锁条件。

### 2026-05-09 — `max_output_tokens` 不发
- codex-rs 的 `ResponsesApiRequest` struct 没这个字段；CC 和其它
  也信任 provider per-model 默认值。leek 对齐。

### 2026-05-11 — rebuild-clean 重置
- 决定删掉 `rebuild` 分支的 agent 后端，在 `rebuild-clean` 上重启。
  诊断：经过 Phase 0a–0g + M1 之后积累了过多的确定性系统脚手架
  （routing 层、deliverable 分类、plan_guard、task 实体）；M1
  QA 期间每个"救火"修改产出的是更多架构纠缠，而不是更少。
- 重置之后保留的：横向原则、locked 设计决策（idle / wall-clock
  / doom-loop / auto-compact / 等等）、milestone 顺序（加了 M0；
  M1–M4 保留）。
- 删除的（字面清单）：见 `ARCHITECTURE.md §10`。
- 新增的：`M0 — Clean skeleton`（在重新实现 M1 之前，把分支手术
  本身做成一个显式 milestone）。

### 2026-05-11 — 不抽象 LLM provider
- 当前只有一条路径：codex pro OAuth → Responses API。用户没有
  第三方 API key；没有第二个具体 provider 可以基于它做设计。
- 老的 `LlmProvider` trait 是投机性的。rebuild-clean 里移除。
  等真的第二个 provider 出现时再抽象。

### 2026-05-11 — Subagent 从 M0 进架构、机制放在 M2.7
- ARCHITECTURE.md 描述 subagent 是端态 agent 拓扑的一部分（§4.2）。
  MILESTONES.md 仍然把*机制*实现放在 M2.7，因为：(a) M1 不需要
  subagent 也能证明 loop 通；(b) M2.7 合理依赖 M2.5 的 skill
  机制做 persona 绑定。架构上从第一天起就是多 agent 概念，但
  spawn 机制在 M2.7 才到位。
