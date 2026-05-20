# L.E.E.K 重建路线图

> **rebuild-clean 分支的目标和顺序的源头依据。**
>
> 配套文档：`docs/REQUIREMENTS.md` 与 `docs/ARCHITECTURE.md`。
> REQUIREMENTS 告诉你产品 / UX / 验收边界；ARCHITECTURE 告诉你
> 端态形状；本文档告诉你到达端态的顺序。
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
- 产品 / UX 边界放在 `REQUIREMENTS.md`；本文只描述阶段切分。
- 文末的 Decision log 记录的是 locked 决策*为什么*被 lock。

---

## 状态标识（2026-05-19）

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
- **M1 completed（2026-05-19）**：echo worker 已替换为真实主 agent
  loop；Codex OAuth、Responses streaming、工具循环、turn_metrics、
  M1 guard set 已接入。收尾子提交 M1.8 auto-compaction 已落地——到阈值
  时摘要压缩早期上下文并继续同一个 turn，`context_limit` 停止分支已移除；
  `LEEK_CONTEXT_WINDOW` env override 让触发窗口可配。gpt-5.5 context
  window 默认值修正为 codex 实际值 272K。M1 没有接 corpus / skill /
  subagent / domain tools——那些是后续 milestone。
  验收：PM 代码审查 + `cargo test` 50/50 + 浏览器 E2E（compaction
  触发 / tool 折叠 / 压缩后继续 / 召回 全部 live 验证）。
- **M1.9 part 1 completed（2026-05-19，M1.9.1–M1.9.5）**：后端 workbench
  事件契约落地——`agent/events.rs` surface 分类（chat/canvas/right_rail/
  lifecycle）+ `CanvasArtifact` 统一信封；`note_trace`、`tool_lifecycle`
  （start/completion/error）、`search_lifecycle`、`plan_updated` 事件；
  tool 三分契约 `model_output / display_payload / debug_payload` + `ToolUi`
  注册表与 `ToolSpec` 分离；`update_plan` 工具；provider-side `web_search`
  （`LEEK_WEB_SEARCH` opt-in，已实测经 codex backend 可用）。
  验收：PM 代码审查 + `cargo test` 68/68 + 浏览器 E2E（6 类事件 + 4 个
  surface 全部 live 验证）。
- **M1.9 part 2 completed（2026-05-19，M1.9.6–M1.9.7）**：前端
  workbench 落地——三栏布局（Sessions │ Chat │ Canvas）；chat 工具
  摘要（运行中逐条显示、完成后聚合、可跳转 canvas）；canvas 按 turn
  分段（note / tool / web_fetch / search cards，失败卡默认折叠可
  展开）；Plan / TODO widget；`store.ts` 按 `payload.surface` 路由。
  同批后端补全 web_search 来源映射 + clippy
  `TurnContext` 重构。M1.9 全部完成。
  验收：PM 代码审查 + `cargo test` 73/73 + `cargo clippy` 0 warning
  + 浏览器 E2E（3 个 turn：plan / web_fetch / 失败工具 / note_trace
  / 19 次 web_search 全部 live 验证）。commit `595ab4a`（后端）+
  `32a1fca`（前端）。
  验收发现：web_search 来源卡片需在 Responses API 请求里带 `include`
  参数才有数据（codex backend 经 `web_search_call.action.sources`
  暴露来源）；M1.9 part 2 当时未带——已由 follow-up commit `cc26f44`
  收尾验收（详见 decision log 2026-05-19）。
  进一步：2026-05-20 M1 用户测试时发现卡片显示问题（同站多页看着像
  重复、`open_page` 被误标成"网页搜索"），又派 follow-up 改用
  `web_search_call.results` 做 `action.type` 分流——commit `cf1d872`，
  见 decision log 2026-05-20。
- **F2 completed（2026-05-20）**：LLM transcript 归档落地——每条
  codex backend 请求（主 iteration + auto-compaction summary call）
  原始 request body + raw SSE response 写进 `llm_transcripts` 表，
  3 个 read API 端点（list 元数据 + raw request + raw response）。
  PM debug 调研"provider 实际发了 / 收了什么"时直接翻 vault.db，
  不再回拨 codex backend。commit `59d65cc`，见 decision log 2026-05-20。

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
| M1.8 | Auto-compaction（summarize and continue）                    | 90%，默认开      |

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
  `(context_window * 9) / 10` 作为阈值。到阈值时摘要压缩旧上下文
  并继续同一个 turn。这是上下文接近上限时的**唯一**设计行为；不设计
  “压缩失败就停 turn”的护栏分支（见 decision log 2026-05-19 第二条）。

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

## M1.8 — Auto-compaction（M1 的收尾子提交）

> 这是 M1 的最后一个子提交（见 M1 sub-commit 表 M1.8 行），**不是
> 独立 milestone**。M1 在 auto-compaction 落地前不算完成。

### 目标

上下文接近窗口上限时，系统自动生成可追溯摘要，替换长历史，然后继续
同一个 turn。用户的问题没做完时默认继续工作，不让用户新开 session。

当前代码里有一个 `context_limit` 停止分支（到阈值结束 turn + 给诊断），
是开发中被擅自加入的工程护栏，不是产品设计。M1.8 用 auto-compaction
**替换**它，不是在它旁边并存。

### Scope

- 触发阈值：默认 90% context window。
- 压缩对象：长 session history、早期 assistant 文本、早期 tool 结果、
  已完成分支；当前用户问题、最近消息、正在进行的 tool call / tool
  result 不得丢。
- 摘要必须保留：
  - 当前目标和用户约束
  - 已确认事实和关键证据
  - tool 结果摘要及 provenance / event refs
  - corpus / web source 引用
  - 未完成分支和下一步意图
  - 已触发 guard / 错误状态
- 压缩结果作为特殊上下文块进入后续 model input；同一个 turn 继续跑。
- 写入可观测事件：`compaction_started`、`compaction_completed`。
- metrics 记录 compaction 次数、压缩前后 token 估算。
- 移除现有的 `context_limit` 停止分支。不设计“压缩失败就停 turn”的
  护栏：压缩是一次模型调用，失败就走通用 `error` 事件路径，跟任何
  模型调用失败一样，不需要专门的停止状态。

### 验收

- 构造长 session 触发阈值后，turn 不停止，而是 compaction 后继续。
- 压缩后模型仍能引用压缩前的关键约束、证据和 tool 结果。
- compaction 事件可在 SSE / event history 中观察。
- 旧的 `context_limit` 停止分支已从代码中移除，不与 auto-compaction
  并存。

---

## M1.9 — Workbench Event Contract：前端/UX 契约

### 目标

把 M1 的后端 loop 事件升级成前端 workbench 可以稳定消费的事件契约。
这是 M2 前的收口 milestone：不接 corpus、不做领域工具，先把 chat /
canvas / right rail 的边界锁死。

### Scope

- `docs/REQUIREMENTS.md` 成为产品 / UX / 验收边界源头。
- `AGENTS.md` 改为优先读 REQUIREMENTS / ARCHITECTURE / MILESTONES；
  `design/` 明确降级为历史参考。
- 事件语义拆分：
  - chat：用户消息、最终 assistant 回复、运行中工具摘要。
  - canvas：Note Trace、tool card、provider-side search card、
    corpus card、subagent card。
  - right rail：Corpus Brain、Plan / TODO。
- tool contract 拆分为 `model_output / display_payload /
  debug_payload`。前端不得从 `model_output` 文本解析业务数据。
- tool UI metadata 与 LLM-facing tool spec 分离。
- `update_plan` 作为展示工具接入：更新 Plan / TODO，不进入 canvas
  tool card，也不是 gate。
- provider-side `web_search` 若当前 Codex backend 可用，则接入并
  映射为 search artifact；不可用时保留契约，不伪造功能。

### Sub-commits（计划）

| #      | 标题                                                                                       |
|--------|--------------------------------------------------------------------------------------------|
| M1.9.1 | 事件语义拆分（chat / canvas / right-rail）+ `note_trace` 事件                              |
| M1.9.2 | tool 生命周期事件统一（start / completion / error）+ `canvas_artifact` envelope             |
| M1.9.3 | tool 三分契约 `model_output / display_payload / debug_payload` + tool UI metadata registry  |
| M1.9.4 | provider-side `web_search` 接入 + search 事件映射（Codex backend 可用时）                    |
| M1.9.5 | `update_plan` 工具 + `plan_updated` 事件                                                   |
| M1.9.6 | 前端：chat 工具摘要（运行中逐条显示，完成后聚合，可跳转 canvas）                             |
| M1.9.7 | 前端：基础 canvas（turn 分段，note / tool / web_fetch / search cards）+ Plan / TODO widget   |

> M1.9.1–M1.9.5 是后端事件契约；M1.9.6–M1.9.7 是前端 workbench。
> Scope 第 1–2 项（REQUIREMENTS 成为权威源、AGENTS.md 重指向）是文档
> 工作，已随 `REQUIREMENTS.md` / `AGENTS.md` 落地，不再单列 sub-commit。
>
> **状态（2026-05-19）**：M1.9 全部完成并验收。M1.9.1–M1.9.5（后端
> 事件契约）单个 commit `2a5dde1`（5 个 sub-commit 互相耦合、无法
> 干净切分）。M1.9.6–M1.9.7（前端 workbench）commit `32a1fca`；
> 同批 `595ab4a` 补全 web_search 来源映射（后端）。
> 验收发现：search 来源卡片需在请求里带 `include` 参数才有数据
> （codex backend 经 `web_search_call.action.sources` 暴露来源）；
> 已由 follow-up commit `cc26f44` 收尾验收——见 decision log 2026-05-19。
> 2026-05-20 进一步 follow-up commit `cf1d872`：`include` 改为
> `web_search_call.results`，按 `action.type` 分流四变体卡片
> （search / open_page / find_in_page / Unknown）——见 decision log
> 2026-05-20。

### 验收

- 旧 `design/` 里的 task / deliverable / mandate / portfolio /
  `LlmProvider` / Reasoning DAG 不能再被新 session 当作当前权威。
- Chat 中只显示用户消息、assistant 最终回复和工具摘要；Note Trace
  不进入 chat 正文。
- Canvas 按 turn 分段展示过程 artifact；最终回复不生成 canvas card。
- 失败工具在 chat 摘要可见；canvas 可以默认隐藏失败卡，但必须能
  展开查看。
- Plan / TODO 只在存在 plan 时显示；plan 不阻止 assistant 回复。

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
- 求证纪律 prompt section：约束 model 对事实问题"先搜后答"，搜不到
  要明说（M1 用户测试 2026-05-20 发现 model 在搜索 0 结果时直接 fallback
  到训练知识，详见 decision log 2026-05-20）

### Sub-commits（计划）

| #    | 标题                                                |
|------|-----------------------------------------------------|
| M2.1 | Corpus loader + BM25 索引，内存中                   |
| M2.2 | `corpus_search` + `corpus_read` 工具                |
| M2.3 | 系统 prompt 默认注入精选 corpus 片段                |
| M2.4 | 求证纪律 prompt section（事实先搜后答）              |

### Design decisions（locked）

- **先 lexical，embedding 后说。** embedding 有 setup 成本，corpus
  编辑还要重算。先 punt，等 lexical 召回有可测量的不足再上。
- **Corpus 用 git 版本管理。** 通过编辑 markdown 文件作者化。
  应用内编辑器是后续 affordance。

### Open questions

- Corpus 更新 reload——v0 启动时加载就够，但什么时候开始想做热
  加载？

---

## M2.1 — Corpus Brain UI

### 目标

让用户看到 corpus 如何参与 agent 的工作，而不是只把 corpus 当文件
列表。右上角常驻 Corpus Brain 显示全 wiki graph，并叠加 agent 实际
使用 corpus 时的激活状态。

### Scope

- Corpus Brain 渲染全 wiki graph（只覆盖 wiki pages，不含 sources）。
- session activation overlay：当前 session 触发过的 wiki 节点叠加
  激活态。
- live activation：当前 turn / 当前工具触发的节点用更强或更短暂的
  激活表现。
- 历史 activation 弱化保留。

### Sub-commits（计划）

| #      | 标题                                                    |
|--------|---------------------------------------------------------|
| M2.1.1 | wiki graph 数据 + Corpus Brain 图谱渲染                 |
| M2.1.2 | session / turn / live activation overlay + 历史弱化      |

### 验收

- Corpus Brain 显示全 wiki graph。
- agent 使用 corpus（`corpus_search` / `corpus_read`）时对应节点激活。
- 历史激活以弱化态保留，当前 turn 激活更突出。

### 依赖

- 依赖 M2（corpus registry + 工具）。
- 依赖 M1.9（canvas / 事件契约——corpus 工具的 artifact 事件）。

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

1. **快速扫描** — "X 现在能不能交易 / 值不值得看？"一个领域取数
   subagent 取数据，主 agent 综合。< 2 分钟 wall-clock。
2. **深度复盘** — 完整个股 review。多个 subagent 并行（领域取数
   + corpus-expert）。主 agent 自己用 `update_plan` 组织步骤。
   5–15 分钟 wall-clock 典型。
3. **对比** — N 个 ticker。N 个并行领域取数 subagent，主 agent 综合。

> 领域取数 subagent 的具体名字和工具子集 M3 启动时定（见
> `ARCHITECTURE.md §4.2.2`：不提前写死）。

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
- 这一版只落地了 `context_limit` stop fallback，没有完成真正
  auto-compaction。2026-05-19 已修正口径：fallback 不能算 M1.8 done。
- `web_fetch` 是 M1 的通用验证工具，不是领域工具。它只允许
  HTTP(S)，并阻断 localhost / private IP literal 这类本机和内网
  入口；更完整的 DNS rebinding 防护等到多用户或远程部署前再补。

### 2026-05-19 — Auto-compaction 口径纠正
- 用户纠正：context-limit stop-only 不是合理产品目标。codex /
  Claude Code 类 agent 在上下文接近上限时应当自动压缩并继续工作；
  事情没做完时，默认应该继续，而不是停下来要求用户精简或新开 session。
- 决定：M1.8 恢复为真正 auto-compaction（summarize-and-continue）。
  当前代码里的 `context_limit` 停止暂留为 fallback，不能算 M1.8
  完成。
- 影响：M1 改成 “core completed；M1.8 pending”。M1.9 workbench
  event contract 仍然保留，但应排在 M1.8 full auto-compaction 之后。
- **后续修订（同日，见下方“context-limit fallback 整个移除”条）**：
  此条里“`context_limit` 暂留为 fallback”已被推翻——fallback 概念
  整个删除，不与 auto-compaction 并存。

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

### 2026-05-19 — context-limit fallback 整个移除：codex 擅自加的工程护栏
- 用户指出：当前代码里的 `context_limit` 停止分支（上下文到阈值就
  结束 turn + 给诊断）**不是产品设计**，是 codex 在用户不知情时
  擅自加入的工程护栏。用户原本的设计只有 auto-compaction，没有这种
  停 turn 的兜底。
- 背景：这次 rebuild 的根本原因之一，就是 codex 在旧版本开发时擅自
  加了大量工程护栏。确定性护栏在 non-deterministic 的 agentic system
  里不解决问题，只会让 agent 行为机械化、抹掉 LLM 的优势。
- 决定：`context-limit fallback` 概念整个移除。上下文接近窗口上限时
  的**唯一**设计行为是 auto-compaction（摘要压缩后继续同一个 turn）。
  不设计“压缩失败就停 turn”的护栏分支——压缩是一次模型调用，失败
  就走通用 `error` 错误路径，不需要专门的 `context_limit` 停止状态。
- M1 收尾要求：当前代码的 `context_limit` 分支必须被 auto-compaction
  **替换**，不是并存。M1 在此之前不算完成。
- 影响：
  - REQUIREMENTS §0（删 `context-limit fallback` 术语）、§7.1
    （章节改为纯 Auto-compaction）、§9 M1 验收 + 实现状态
  - ARCHITECTURE §5（guard 表 Auto-compaction 行去掉兜底措辞）
  - MILESTONES 状态标识、M1 “Auto-compaction 90%” locked 决策、
    M1.8 子提交章节
- 同类审查：`doom-loop detector` 在 ARCHITECTURE §12.7 原则下是
  同类嫌疑最大的 guard（leek 自创、codex / CC 都没有、默认开）。
  已 surface 给用户。**用户 2026-05-19 决定：doom-loop 保持当前
  设计（默认开、N=3）不动**；idle timeout / wall-clock 一并保留
  现状。`context-limit fallback` 的移除不波及其它 guard。

### 2026-05-19 — leek 维护独立的 codex OAuth 凭证（auth login，不 import）
- 用户决定：leek 必须用自己的 `auth login`（device-authorization
  flow）走一遍 OAuth，拿到并维护**自己独立**的 token set，不要用
  `auth import` 去复制 codex CLI 的 `~/.codex/auth.json`。
- 理由：codex OAuth 的 refresh token 是 single-holder。`auth import`
  会让 leek 和 codex CLI 共享同一个 refresh token——谁先 refresh
  就 rotate 掉它，另一方失效后无法再 refresh。各自独立登录、各自
  维护，就没有互相踩踏。
- 影响：leek vault 的 `auth_tokens` 存的是 leek 自己登录得到的
  token，与 codex CLI 完全隔离。验收 / 部署一律用 `auth login`。
- 待决（独立小任务）：代码里的 `auth import` 子命令现在是个 footgun，
  建议删除，只留 `auth login`。本次未动。

### 2026-05-19 — codex context window 调查 + leek 窗口/阈值 override 设计
- 调查：codex 的 per-model metadata 来自后端 `/models` endpoint，
  缓存在 `~/.codex/models_cache.json`。gpt-5.5 的实际值：
  `context_window=272000`、`max_context_window=272000`、
  `effective_context_window_percent=95`、`auto_compact_token_limit=None`。
- 结论：**gpt-5.5 经 codex 的 raw context window = 272K**。leek
  `pricing.rs` 里硬编码的 400K 是 leek 侧的错误猜测。
- codex 的窗口模型（源码 `protocol/src/openai_models.rs`、
  `core/src/session/turn_context.rs`）：可用输入窗口 = raw ×
  `effective_context_window_percent`(95%) ≈ 258.4K；auto-compact
  触发点 = `(context_window × 9)/10` = raw × 90% ≈ 244.8K。
  gpt-5.5 的 `max_context_window` 也是 272K → codex 不允许把
  gpt-5.5 override 到 272K 以上（对比 gpt-5.4 的 max 是 1M）。
- leek 设计决策（用户 2026-05-19 拍板）：
  - context window 默认对齐 codex：gpt-5.5 = **272K**（`pricing.rs`
    的 400K 要改成 272K——独立小任务，M1.8 完成后派）。
  - context window 可 env override：`LEEK_CONTEXT_WINDOW`（含在
    M1.8 spec scope #8）。对 gpt-5.5 实际只用于往下调（测试）。
  - compaction 阈值默认 0.90（已对齐 codex 的 ×9/10），可 env
    override：`LEEK_AUTO_COMPACT_THRESHOLD`（已存在）。
  - **不**采用 codex 的 `effective_context_window_percent`(95%)
    ——leek 在 90% 就 compact，比 95% 可用上限更早，95% 不会成为
    约束，加它是过度工程。
- 影响：ARCHITECTURE §5（新增 context window / 阈值可配置说明）、
  REQUIREMENTS §7.1（Auto-compaction 小节加配置说明）。

### 2026-05-19 — web_search 来源：codex backend 经 `include` 参数暴露

- M1.9 part 2 验收时实测 web_search（`LEEK_WEB_SEARCH=1`，一个 turn
  跑了 19 次搜索）：search 卡片来源恒为空。抓 codex backend
  （`chatgpt.com/backend-api/codex/responses`）原始 SSE 确认：message
  item 的 `output_text` `annotations` 恒为 `[]`，整条流也没有
  `response.output_text.annotation.added` 事件——codex backend 不发
  answer-level 的 `url_citation` 注解。
- 据此一度结论“codex backend 不通过结构化数据暴露来源、search 卡片
  注定恒空”。**这个结论下早了**——当时没试 Responses API 的
  `include` 请求参数。
- 复测：请求体加 `include` 后，codex backend 把来源按每次搜索发回
  （两个值都实测 HTTP 200）：
  - `include: ["web_search_call.action.sources"]` → 每个
    `web_search_call` item 的 `action` 多出
    `sources: [{"type":"url","url":…}]`。
  - `include: ["web_search_call.results"]` → 多出
    `results: [{"type":"text_result",title,url,snippet}]`（更全，
    但 snippet 每条可达 ~200 词、单次搜索几十条，数据量大）。
- 结论（更正前一条）：web_search 来源**是结构化、可取的**，且按
  `web_search_call` 归属（比 answer-level 注解更准）。leek 当前请求
  没带 `include` 才拿不到。`url_citation`（codex backend 确实不发）
  与 `web_search_call` 上的 sources 是两条不同的东西。
- 收尾（follow-up 已完成验收，commit `cc26f44`）——`build_request_body` 在 web_search 开
  启时加 `include: ["web_search_call.action.sources"]`、`responses.rs`
  解析 `web_search_call` 的 `action.sources`、sources 直接进对应搜索
  的 `search_lifecycle` completion frame（按每次搜索归属，不再需要
  M1.9 part 2 的 turn 级累积 / `last_search` / enriched-frame）。
- `url_citation` 注解解析一并移除：leek 无 provider 抽象、只用 codex
  backend，该 backend 确认不发 `url_citation`，这条路径对 leek 恒死
  ——按 clean-room 不留死代码的原则删掉，`action.sources` 作唯一
  来源路径。将来若接非-codex 的 Responses API，再按需从 git history
  取回。
- M1.9 part 2 代码（`595ab4a`/`32a1fca`）本身无误：search 卡片的
  `sources` 字段与前端渲染 part 2 已铺好，follow-up 只补请求参数 +
  换解析来源。M1.9.1–M1.9.7 + workbench 仍属完成，本条是验收期发现
  的增强，单独派，不重开 M1.9 编号。
- **后续修正（2026-05-20）**：本条结论中"`action.sources` 作唯一
  来源路径"被进一步推翻——M1 用户测试发现 `action.sources` 仅 URL
  无标题、卡片同站多页看着重复；且 `web_search_call.action.type` 不
  止 `"search"`，还有 `"open_page"` 等。改为 `include:
  ["web_search_call.results"]`，一个值覆盖所有 activity，且带标题/
  snippet。详见 decision log 2026-05-20。

### 2026-05-20 — web_search 卡片整顿：用 `results` + 按 action.type 分流

- M1 用户在 workbench 测试时发现 web_search 卡片的两个 UX 问题：
  - **同站不同页看着像重复**：`action.sources` 只给 URL 不给标题，
    卡片 title fallback 到 host——实测一次 ServiceNow 股票代码查询
    返回 27 条结果，9 条 reddit URL 是 9 个不同帖子，但都渲染成 9 行
    "www.reddit.com"，视觉上看着像重复条目。
  - **`open_page` 被误标成搜索**：`web_search_call.action.type` 不止
    `"search"`，还有 `"open_page"`（agent 打开页面读内容）、
    `"find_in_page"`（页面内 pattern 查找）等。leek 一刀切按"网页
    搜索"渲染，`open_page` 没 query/sources 就掉进 "(本次搜索没有
    返回可引用来源)" 这个误导空状态。
- 直连 codex backend 抓 3 个研究 prompt 实测 `web_search_call.action.type`
  的真实形态：`"search"` × 2 + `"open_page"` × 3，每种 `action` 的字段
  不同（`search` 有 `query`/`queries`/`sources`；`open_page` 只有
  `url`；`find_in_page` 文档列了 `url`/`pattern`，本次未触发但已防御
  落地）。关键发现：
  - **`include: ["web_search_call.results"]` 一个值就够** —— `results`
    覆盖所有 activity：对 `search` 是结果列表（`title` + `url` +
    `snippet`），对 `open_page` 是 agent 读到的页面内容片段。
    **`action.sources` 是 `results` 的 URL-only 子集，整丢**（直接推翻
    了 2026-05-19 决定的"`action.sources` 作唯一来源路径"）。
  - `results[i].snippet` 带 codex 内部头需剥：`【turnXkindN】
    [wordlim: N] Published: …; Crawled: …;`（search）或
    `Content type: …; Source: open(…); Total lines: N\nL<N>: …`
    （open_page，body 每行带 `L<N>:` 行号前缀）。
- 用户拍板四变体卡片设计（2026-05-20）：
  - **search** 卡：`网页搜索 · <query> · N 条结果`，top 6 标题列表
    （整行可点跳）+ "显示全部 N 条"展开；
  - **open_page** 卡：`打开网页 · <host>`，标题（无标题则 URL，均可
    点跳）+ ~200 词 cleaned snippet 默认展开；
  - **find_in_page** 卡：`页面内查找 · <pattern> · <host>` + URL +
    匹配段落默认展开；
  - **Unknown**：`网页活动 · <type>` + 防御性 JSON 字段 dump。
- 实现（commit `cf1d872`）：
  - 后端：`include` 切到 `results`、`responses.rs::web_search_event`
    按 `action.type` 分流到 `WebSearchAction` enum 四个变体（Search /
    OpenPage / FindInPage / Unknown）、新增 `clean_snippet` 剥前缀、
    `CanvasArtifact::search` 签名通用化为接受 `data: Value`、单测覆盖
    四种 action.type + clean_snippet 各种头格式。
  - 前端：`Artifact` 加 `actionType` + 变体字段、`Canvas.tsx`
    `renderSearchCard` 顶部 const 全改 accessor + 结构性 `if` 换
    `<Switch>/<Match>`（Solid store 字段只在 accessor 上下文才被追踪
    ——否则 start→completion 切变体不响应，要 turn 折叠重 mount 才
    显示；`find_in_page` 触发了这个 bug，PM E2E 暴露并就地一起修）。
  - 删旧 "(本次搜索没有返回可引用来源)" 空状态文案。
- 后端事件契约：`search_lifecycle.data` 现按变体载荷，顶层一律带
  `action_type` 字段供前端分流；事件 `kind` 保持 `search_lifecycle`，
  对外契约稳定。
- PM 浏览器 E2E 验收：`LEEK_WEB_SEARCH=1` 跑研究 prompt 触发 search +
  2×open_page + find_in_page，canvas 三种变体全部 live 自动渲染正确，
  find_in_page 不再卡在"搜索中…"。`cargo test` 78/78、`cargo clippy`
  0 warning、前端 build 通过。
- **F2 follow-up（memory 已记）**：用户提出 leek 应**归档每 turn 的
  原始 LLM transcript**（codex backend 请求体 + raw SSE 响应），让
  debug 直接翻档案、而不是每次叫 PM 重抓。本次卡片整顿期间多次直连
  codex backend 抓 SSE 让用户察觉到这个 observability 缺口，作为独立
  follow-up 推进——属于数据持久化基础设施，跟显示无关，不裹在本任务。

### 2026-05-20 — 求证纪律（search-first prompt discipline）——延后到 M2.4

- M1 用户测试发现：问 "$NOW 是什么股票" 时，model 发了一次搜索
  query `"finance: NOW"`，codex backend 字面对待 `finance:` 不识别的
  operator、返回 0 结果。model 没换 query 重试，直接用训练知识答出
  "$NOW = ServiceNow"——答案碰巧对，但跳过了 web 求证。
- 用户提出 leek 不应直接用世界知识回答事实问题——即使答案对，投研
  agent 用户也无法分辨哪些是搜的、哪些是脑补的；且训练知识有过时
  / 幻觉风险。
- 这是 system prompt / harness behavior 问题：current `OPERATING`
  章节（`crates/gateway/src/agent/prompt.rs`）对工具使用是 permissive
  的（"需要外部信息或要执行动作时调用工具"），没有强制"事实必查"。
- 用户决策（2026-05-20）：**这是 harness 问题，延后到 M2 处理**（M2
  本就要扩 system prompt）。新增 M2.4 sub-commit 落地"求证纪律"
  section。先记本条，M1 用户测试不被这条 block。
- 设计 sketch（待 M2.4 时确认细节）：在 `prompt.rs` 的 `OPERATING`
  与工具清单之间加一段 `# 求证纪律`。约束：
  - 事实问题（代码/价格/新闻/事件/日期/数字等）**先搜后答**；
  - 一次搜 0 结果或不相关 → 换 query 重试 1–2 次；
  - 仍搜不到 → 明说"搜不到"；若用户仍要训练答案，显式标注「来自
    训练知识，无法用搜索证实，可能过时」；
  - 分析、判断、推理不受约束，但其**事实依据**必须搜过、可追溯。
- 严格度档位（M2.4 实现时再选）：当前 sketch 是"搜不到可加 caveat 后
  给训练答案"。更严的档是"搜不到就停、不给训练答案"。当时再敲。

### 2026-05-20 — F2:LLM transcript 归档（per-iteration 原始请求 + SSE 响应）

- 背景：M1 用户测试期间，PM debug 多次为"`web_search_call` 长啥样"
  这种问题直连 codex backend 抓 raw SSE——leek 当前只持久化
  `events` 表（已处理后），没保存原始 LLM 层。每次 debug 都得重新
  消耗 token、复现状态难。
- 用户反馈（memory `feedback_transcript_archive`）：leek 应按
  turn × iteration 归档原始 LLM transcript，debug 直接翻档案。
- 实现（commit `59d65cc`）：
  - **Schema**：新 migration `0004_llm_transcripts.sql`——表加
    `request_body` BLOB（leek 构造的请求 JSON verbatim，Authorization
    header 不在 body，无凭证泄漏）+ `response_stream` BLOB（raw SSE
    `event:` / `data:` 框架 verbatim）+ `http_status`（200 / 4xx-5xx /
    0 sentinel 表 stream 中断）+ started_at / finished_at；
    `(turn_id, iteration)` UNIQUE；FK 到 sessions ON DELETE CASCADE。
  - **写入**：`vault::llm_transcripts::insert_request` 在 POST 前
    eagerly 插入（crash 容灾），`finalize` 在 stream 终止时一次写入
    response_stream + status + finished_at；`codex::chat` 用 RAII
    `TranscriptGuard` 在 stream 自然结束 / 提前 drop（idle_timeout
    取消、consumer cancel）时 tokio::spawn finalize，`completed`
    atomic flag 区分 200 vs 0 sentinel；非 2xx 立刻 finalize 错误
    响应体。
  - **请求归属**：`ChatRequest` 加 `session_id` / `turn_id` /
    `iteration` 三字段，`build_request_body` 不读、不上 wire；agent
    loop 用独立的 `transcript_iter` 计数器（含 compaction sub-call，
    与 `turn_metrics.iteration_count` 解耦），每个 LLM call 占一行。
  - **API**：`GET /api/v1/sessions/{id}/transcripts` list（元数据 +
    sizes 不含 raw bytes）；`GET .../{turn_id}/{iteration}/request`
    返回 `application/json` raw bytes；`GET
    .../{turn_id}/{iteration}/response` 返回 `text/event-stream` raw
    bytes——`curl | jq` / `curl | grep` 直读不用 base64。
  - **解耦**：`parse_sse_stream` 改泛型签名 take bytes stream，方便
    `codex::chat` 在外层 tap 字节。
- 不做（MVP）：不动 `events` 表、不动前端、不做 retention / 压缩、
  不做 chunk-level eager finalize。
- 单测：vault 5 个（round-trip / UNIQUE / 排序 / session scoping /
  unfinalized）+ api 5 个（list 空+含 / raw bytes Content-Type / 404）。
  cargo test 78→88（+10），clippy 0 warning，前端 build 通过。
- PM curl E2E：
  - happy path（"用一句中文打招呼"）：~4s 完成，http_status=200，
    response_bytes=20725，末尾到 response.completed event 完整。
  - idle_timeout：web_search 长 turn 触发 90s idle_timeout，
    http_status=0 sentinel + partial bytes 准确归档（被杀前的数据
    完整保留）。
  - goal use case（"NVDA 在哪个交易所"）：~10s 完成，
    response_bytes=48436，档案里直接 grep 到
    `"web_search_call","status":"completed","action":{"type":"search",
    "queries":[...]}` + `"results":[...]`——以后调研 web_search_call
    形状不需要回拨 codex backend，闭环用户反馈。
- 已知约束（spec 明确）：
  - status=0 既可能是 idle_timeout 杀流、也可能是上游 transport
    error，schema 不区分；PM 看 buffer 末尾是否到 response.completed
    即可判。
  - 没做 retention，数据量大了再说（可能 follow-up 加 gzip BLOB /
    周期截断）。
  - finalize 是 tokio::spawn 异步，turn 结束后立刻 GET 可能瞬间看到
    http_status=null；等 ~百 ms 即落盘。
- 闭环 memory `feedback_transcript_archive`（2026-05-20 创建）。后续
  PM 调研路径变了：先 vault.db / API，找不到再考虑直连 codex backend。
