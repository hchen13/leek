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

## 状态标识（2026-05-18）

老的 `rebuild` 分支完成了 M1 + Phase 0g 清理，之后被诊断为携带
了过多的确定性系统脚手架（routing 层、deliverable 分类、plan_guard），
在预算内无法挽救。2026-05-11 的方案是删掉 agent 后端、保留前端
和部分基础设施，在 `rebuild-clean` 上重启。

2026-05-18 复盘后进一步收紧：**partial-retain 风险仍然太高**。
旧 runtime 能编译、旧前端能 typecheck，但 migration、API、事件模型、
README 和 UI 仍会把 `tasks` / `deliverables` / `mandate` /
`portfolio` / `compaction` 等旧概念带回 active path。M0 改为
**clean-room rebuild**：只保留设计资产和内容资产；runtime、migration、
frontend active tree 全部清底重写。旧代码通过 git history 查询，
不放进编译路径、路由路径、migration 路径或前端入口。

老的 milestone 完成标记（M1 DONE 等）有意被重置——*实现*它们的
*代码*不再继承。它们*验证过的原则*仍然 locked。

当前 `rebuild-clean` 实现状态：

- **M0 completed（2026-05-18）**：runtime / migration / frontend
  active tree 已 clean-room 重写，HTTP + SQLite + SSE echo 骨架可跑。
- **M1 completed（2026-05-18）**：echo worker 已替换为真实主 agent
  loop；Codex OAuth、Responses streaming、工具循环、turn_metrics 和
  M1 guard set 已接入。注意：M1.8 落地的是 context-limit guard
  （到阈值停止并给诊断），不是摘要压缩后继续工作的完整 auto-compaction。
  M1 没有接 corpus / skill / subagent / domain tools。

---

## M0 — Clean-room skeleton（清底骨架）

### 目标

立起最小端到端可用的纵切片，用来证明 plumbing 通了：能开 session、
发消息、通过 SSE 收到响应。**这个阶段没有 agent loop，没有工具，
没有 LLM 调用，没有 OAuth 登录要求，没有前端产品壳。** "响应"是
服务端 echo 回来。M0 证明的是全新 runtime 的最小 HTTP / SQLite /
SSE 闭环，不是复活旧应用。

### Scope

- Runtime 清底：`crates/gateway/src/` 不继承旧 active code。重建
  一个最小 Rust binary：CLI 参数、Axum server、SQLite open/migrate、
  health、session CRUD、message POST/list、SSE stream、echo worker。
  旧 OAuth / LLM / corpus / event / static file 代码一律暂不接回。
- Migration 清底：删除老的 0001–0010；新建唯一
  `0001_initial.sql`，只覆盖 M0 schema：`users`、`sessions`、
  `messages`、`events`。如果 M0 没有实际字段使用，连
  `user_settings` 也不要加。
- Frontend 清底：`frontend/web/src/` 不继承旧 active UI。M0 可以
  没有前端；如果要验证浏览器端，只写一个极简 chat harness，不能
  带 portfolio、decision、plan、corpus、compaction、settings、
  canvas fixture 等旧产品壳。
- Legacy quarantine：旧代码不搬到 `tmp/legacy`，避免制造第二份
  状态源。需要参考时用 `git show <old-rev>:<path>` 或
  `git grep <symbol> <old-rev>` 精确摘取，再按新架构重写。
- 验证：用 curl 或极简前端发一条消息，看 user message 和 echo
  assistant message 都持久化，SSE 收到 `message_created` /
  `assistant_delta` / `assistant_done` 等最小事件。没有 agent，
  没有 LLM。

### Sub-commits（计划）

| #    | 标题                    | Scope                                                                                 |
|------|-------------------------|---------------------------------------------------------------------------------------|
| M0.1 | Runtime wipe            | 清空旧 active runtime；只留下最小 crate/module 骨架和必要依赖                           |
| M0.2 | Vault schema v1         | 单一 `0001_initial.sql`：users / sessions / messages / events                           |
| M0.3 | HTTP + SSE echo         | Session CRUD；POST 消息；后台 echo assistant；事件持久化 + SSE fan-out                  |
| M0.4 | Verification harness    | curl 脚本或极简前端；证明消息持久化、事件流、重启后 list 可恢复                         |
| M0.5 | Docs sync               | README / ARCHITECTURE / MILESTONES 同步为 clean-room 状态，删掉旧 quickstart 误导         |

### Design decisions（locked）

- **M0 用单一 migration 文件**，不做历史保留。我们不保留老 vault
  格式。新用户用新 schema。
- **M0 用 echo，不调 LLM**。把 plumbing 和 agent 逻辑分开能让 M0
  小、让 M1 端到端地拿到模型集成的所有权。
- **不存 `tasks` 表。**会话就是消息序列。见 `ARCHITECTURE.md §6`。
- **M0 不保留旧前端作为产品基础。**旧 UI 的视觉和组件可以之后从
  git history 摘取，但 M0 active frontend 只能服务 echo 验证。
- **M0 不保留 OAuth / LLM client。**M1 从 git history 精确摘取
  codex OAuth / Responses parsing 的有效片段，摘取时必须删掉
  `LlmProvider` trait 和 routing/compaction/subagent surface 残留。

### Open questions

- 开发机上现存的 vault DB 默认丢掉；M0 不写 migrator。
- M0 是否需要前端：默认不需要。若为了人工验收写极简前端，它必须
  独立于旧产品 UI，且不能扩大 M0 scope。

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
| M1.8 | Context-limit guard（auto-compact threshold only）           | 90%，默认开      |

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

- **Context-limit guard 90%** — 对齐 codex 的硬编码
  `(context_window * 9) / 10` 作为阈值。M1 只保证到阈值时不继续把
  context 撑爆：停止 turn、持久化诊断消息和 metrics。真正的
  summarize-and-continue auto-compaction 不算 M1 已完成内容。

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
- Cost cap 多档价格初版已在 M1 落地为 `input / cached input /
  output` 三档估算；codex backend 没有公开价格面，后续以真实账单或
  vendor 价格更新 `pricing.rs`。

---

## M2 — Corpus

### 目标

把让 leek 区别于通用 agent 的核心内容搞起来：投资 **corpus**。
通过主 agent 的 system prompt 默认注入 + 工具按需检索两条路径
暴露。

> **注**：之前 M2 还包括"用户 mandate"，2026-05-11 整个移除，
> 推迟到 leek 跑起来之后的独立 memory 课题（详见 ARCHITECTURE
> §7 + 本文件 decision log）。

### Scope

- Corpus loader：从一个根目录读 markdown，做 lexical（BM25）检索
- 工具：`corpus_search(query)`、`corpus_read(id)`
- 默认 corpus 注入到主 agent 的 system prompt（目标 < 800 tokens）

### Sub-commits（计划）

| #    | 标题                                                |
|------|-----------------------------------------------------|
| M2.1 | Corpus loader + BM25 索引，内存中                   |
| M2.2 | `corpus_search` + `corpus_read` 工具                |
| M2.3 | 系统 prompt 默认注入精选 corpus 片段                |

### Design decisions（locked）

- **先 lexical，embedding 后说。** embedding 有 setup 成本，corpus
  编辑还要重算。先 punt，等 lexical 召回有可测量的不足再上。
- **Corpus 用 git 版本管理。** 通过编辑 markdown 文件作者化。
  应用内编辑器是后续 affordance。

### Open questions

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

- Plugin 沙箱——第一版不做；只信本地安装。
- Skill 的 model override 跟 codex OAuth 组合时怎么解（CC 每个
  skill 可换 model；codex pro 就一个 model）。

---

## M2.7 — Subagent

### 目标

一个通用机制，用来 spawn 一个有自己 context window、system
prompt、工具子集的子 agent loop。投资领域真的吃这个（多 ticker
并行扫描、corpus-expert 委派、把重活赶进独立 context）。topology
的理由见 `ARCHITECTURE.md §4.2-4.3`。

### Scope

- 一个 `task` 工具（CC 约定）给主 agent 调用
- subagent 在自己的 loop 里跑：自己的 system prompt（来自
  `AGENT.md` body）、自己的工具子集（来自 `AGENT.md` frontmatter
  的 `allowed_tools`）、自己的消息历史
- **subagent loop 复用所有 M1 guard**（cost cap / wall-clock /
  idle / iteration / doom-loop / turn_metrics）
- 结果作为单个 text block 返回给父级（v0 不流式）
- **AGENT.md 驱动的 persona 绑定**（agent 跟 skill 严格分开维护，
  见 `ARCHITECTURE.md §4.2`）：`task(agent_name="corpus-expert",
  input="...")` 加载 `harness/agents/<name>/AGENT.md`，body 作为
  subagent 的 system prompt，frontmatter `allowed_tools` 作为工具
  子集。
- 嵌套：subagent 可以 spawn subagent，默认 depth 上限 2（主 →
  子 → 孙就停）

### Sub-commits（计划）

| #       | 标题                                                                          |
|---------|-------------------------------------------------------------------------------|
| M2.7.1  | `task` 工具 + subagent loop spawn                                             |
| M2.7.2  | AGENT.md loader + frontmatter 解析（三层路径发现）                              |
| M2.7.3  | Depth 上限 + per-subagent turn_metrics 行（parent_turn_id 链接）                |
| M2.7.4  | 内置 AGENT.md：`general-purpose`（基线）+ `corpus-expert`（领域 subagent 等 M3） |

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
   + corpus-expert）。主 agent 自己用 `update_plan` 组织步骤。
   5–15 分钟 wall-clock 典型。
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
- 默认开：idle timeout、wall-clock、doom-loop、context-limit guard、
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
  本身做成一个显式 milestone）。此条里的 partial-retain 执行方式
  已被 2026-05-18 的 clean-room 决策替代。

### 2026-05-18 — M0 从 partial-retain 改成 clean-room rebuild
- 用户担心"删一部分留一部分"的重构成本和方向风险太高：旧代码能
  编译、旧前端能 typecheck，但 schema/API/UI/文档仍会继续携带旧
  mental model。
- 决定：M0 不再保留旧 runtime / migration / frontend active tree。
  只保留设计资产和内容资产。旧实现作为参考留在 git history 里，
  不复制到 `tmp/legacy`，避免出现第二份状态源。
- M0 端态收缩为：最小 Rust server + SQLite v1 schema + session /
  message API + SSE echo + curl/极简前端验证。
- M1 开始再按需从 git history 摘取 OAuth / Responses parsing 等
  具体片段，摘取时必须删除旧 `LlmProvider`、routing、compaction、
  subagent surface 和 M0 不需要的 schema。

### 2026-05-18 — M1 clean-room agent loop 落地
- M1 在 clean-room runtime 上重新实现，没有继承旧 `LlmProvider`
  trait、routing、task/deliverable、plan_guard 或旧 compaction
  subsystem。
- 落地范围：Codex OAuth、Responses streaming、主 agent loop、
  `echo` + `web_fetch` 工具注册、`turn_metrics`、M1 guard set、
  SSE 事件扩展和前端 harness 的流式气泡。
- M1.8 的名称修正为 `context-limit guard`：它只做 90% 阈值检测
  + 诊断性停止；不做摘要压缩后继续。真正 summarize-and-continue
  不能算 M1 done，后续要作为独立 milestone/commit 设计和验收。
- `web_fetch` 是 M1 的通用验证工具，不是领域工具。它只允许
  HTTP(S)，并阻断 localhost / private IP literal 这类本机和内网
  入口；更完整的 DNS rebinding 防护等到多用户或远程部署前再补。

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

### 2026-05-11 — Subagent 配置机制：AGENT.md（与 skill 分开维护）
- 调研 CC（`~/research/repos/claude-code-sourcemap/restored-src/`）+
  codex（`~/research/repos/codex/codex-rs/`）的 subagent 实现，
  确认两边都是"同一份 loop 代码 + 不同 system prompt + 不同工具
  子集"的抽象层。
- 关键发现：codex 的 `laplace` / `fermat` / `lagrange` / `russell`
  **不是 subagent 标识**，是 spawn 时从 101 个科学家名字池里随机
  挑一个当 display name（证据：`codex-rs/core/src/agent/agent_names.txt`）。
  codex 的 subagent 真正概念叫 `role`（`core/src/agent/role.rs`），
  内置 default / explorer / worker / awaiter。
- CC 的 subagent 跟 leek 之前提议的"skill body 当 subagent system
  prompt"格式上完全兼容——CC 的自定义 agent 就是 YAML frontmatter
  + markdown body。
- 决定：照搬 CC 的文件格式（AGENT.md = frontmatter + body），
  但与 skill **严格分开维护**：
  - 内置 agent：`harness/agents/<name>/AGENT.md`
  - 用户全局：`~/.leek/agents/<name>/AGENT.md`
  - 项目级：`<project>/.leek/agents/<name>/AGENT.md`
  - 三层都进 agent 注册表，同名优先级"项目 > 用户 > 内置"
- 为什么分开（用户决策）：skill 是注入主 agent system prompt 的
  上下文展开式内容（`use_skill` 注入到当前 loop），agent 是被
  `task()` spawn 出去带独立 loop 的子 agent。混在一起会让 subagent
  被主 agent 误当 skill，调用方式混乱。两者 frontmatter 里
  description 的写法也不一样（skill 是"包含什么知识"，agent 是
  "能做什么委派工作"）。
- 同时 lock skill 的三层路径模型：内置 `harness/skills/` + 用户
  全局 `~/.leek/skills/`（第三方 skill 通过界面安装到这里）+
  项目级 `<project>/.leek/skills/`。

### 2026-05-11 — 用户 mandate 整个移除，推迟到 memory 课题
- 用户拍板：之前 M2 设计的"用户 mandate"（一段 markdown 注入
  system prompt、user_settings 持久化、onboarding skill 收集）
  **整个移除**。
- 理由：harness 里的"用户 mandate"本质上是 **memory** 的一个特例
  ——持仓 / 风格 / 风险偏好 / 跨 session 持久化的语义状态。memory
  是一个独立的设计课题：要分层（项目级 / 用户级 / session 级，
  类似 CC 的 CLAUDE.md + project memory + user memory）、要有
  编辑 UX / 冲突解决 / 版本管理。CC / codex 的最佳实践还在演进
  中，硬塞一个半成品 mandate 就是又埋一个 deterministic-systems
  包袱。
- 合理时机：M3 A 股 MVP 跑起来之后看真实痛点，再回到这个课题，
  作为独立 milestone（暂称 M5）。届时会重新调研当时的 CC / codex
  memory 实现作为参考。
- 影响：
  - ARCHITECTURE.md §1（差异化清单只剩 corpus + 领域工具）
  - ARCHITECTURE.md §2（vault 不存任何用户特定语义数据）
  - ARCHITECTURE.md §4.1（system prompt 顺序从 5 项缩到 4 项）
  - ARCHITECTURE.md §7（替换为"已移除"存根）
  - ARCHITECTURE.md §11（删除"用户 mandate (M2)" open questions）
  - MILESTONES.md M2（"Corpus + Mandate" → "Corpus"，删 M2.4/5/6
    三个 sub-commit）
  - MILESTONES.md M2.5（删 mandate-as-skill open question）
  - MILESTONES.md M2.7（删 subagent mandate 可见性 open question）

### 2026-05-18 — 内置 subagent roster 调整：去 planner、加 general-purpose
- 用户反馈两点。
- 去 `planner`：计划是主 agent 用 `update_plan` 工具就地做的事，
  不是委派出去的事。单独 spawn 一个"只产出 plan 不执行"的 subagent
  只是多一个来回 + 一次 context 交接，没有收益。若做计划本身需要
  调研，那部分调研委派给 general-purpose / corpus-expert 即可。
- 加 `general-purpose`：这是 subagent 机制的"无专业化"基线形态
  ——全工具集、通用 worker system prompt、`task()` 不指定 agent
  时的默认。CC 也有这个内置。其它专业 subagent 本质上就是
  general-purpose + 受限 prompt + 工具子集。架构上必须有。
- `market-data-fetcher` 从"rebuild-clean 初始三个"里挪走：它依赖
  M3 的领域工具，归 M3。ARCHITECTURE §4.2.2 不再把它列为近期内置。
- 影响：
  - ARCHITECTURE.md §4.2.2（改写：general-purpose 基线 +
    corpus-expert 专业化 + 领域 subagent 留到 M3；显式写明不做
    planner）
  - MILESTONES.md M2.7.4（内置 AGENT.md 改为 general-purpose +
    corpus-expert）
  - MILESTONES.md M3 深度复盘 task 形态（去掉 planner subagent，
    改为主 agent 自己 update_plan）
- 顺带确认：ARCHITECTURE.md 不规划具体的金融投研工具清单（只有
  §1 一句品类级描述）。具体工具名（`market_quote` 等）只在
  MILESTONES M3。
