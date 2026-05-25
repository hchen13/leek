# L.E.E.K (老韭菜)

**Logic-Enhanced Equity Kernel** — an investment research operating system for the long-suffering retail investor who wants to stop being market fodder.

## Project Identity

L.E.E.K is a **gateway-style agent system** (long-running daemon + multiple adapters: CLI / web / MCP HTTP / TUI / Claude Code skill / ACP) that turns a curated investing-wisdom corpus into actionable research, decisions, and post-mortems.

The CLI binary is named `leek`.

## Project Shape

L.E.E.K is not just an investment prompt collection. It is a **harness** for
corpus-grounded investment agents: the durable work is in loop control, tool
surfaces, state, evidence, provenance, plan semantics, budgets, recovery, and
human confirmation boundaries.

Implementation state should be read from the code. Product and UX boundaries
are locked in [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md). End-state
architecture is in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), and
milestone order / completion state is in [`docs/MILESTONES.md`](docs/MILESTONES.md).
The old [`design/`](design/) tree is historical reference only unless a current
docs file explicitly re-adopts a detail from it.

## Relationship to `finance-giant/` and the corpus

The two projects are **deliberately separate repos sitting side-by-side** at `~/playground/`:

```
~/playground/
├── finance-giant/           # corpus + raw-material collection (this is NOT leek)
│   └── corpus/              # the LLM wiki — read-only data layer for leek
└── leek/                    # this project — the agent system
    ├── AGENTS.md
    └── design/
```

- **`finance-giant/corpus/`** is universal, slow-moving, curated investing wisdom (Buffett / Munger / Dalio at minimum). It is L.E.E.K's *read-only* knowledge layer.
- **`leek/`** is the agent system: gateway daemon, adapters, scheduler, decision/position tracker, promotion pipeline.
- Per-user runtime state (decisions, holdings, reviews, mandates) lives in a separate **vault**, never in the corpus and never in this repo.

**The agent never writes directly to `corpus/wikis/` or `corpus/sources/`.** Promotion goes through `corpus/inbox/` with human review. This is multi-user-safety scaffolding even in solo mode.

## Cold-starting an agent session here

When opening a new agent session in this project, do this in order:

1. Read this file (`AGENTS.md`).
2. Read [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md), then the current code paths relevant to the task.
3. Use [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for architectural principles and [`docs/MILESTONES.md`](docs/MILESTONES.md) for phase boundaries.
4. Treat `design/` as archive material. Do not follow old task / deliverable / mandate / portfolio / `LlmProvider` / Reasoning DAG specs unless the current docs explicitly say to reintroduce that idea.
5. If `~/playground/finance-giant/corpus/` exists locally, glance at `finance-giant/corpus/AGENTS.md` to understand the corpus shape; otherwise the GitHub repo is `hchen13/the-corpus`.
6. Reference repos used as architecture sources may be at `~/research/repos/` (`dexter`, `warp`, `FinceptTerminal`, `hermes-agent`); clone if needed when the discussion calls for them.
7. For agent-loop, plan, tool, subagent, skill, budget, context, or reliability work, read [`.agents/skills/harness-engineering/SKILL.md`](.agents/skills/harness-engineering/SKILL.md) before proposing changes.
8. Resume from the current task request and current code state.

## 与用户沟通的协议(跨 session / 跨 harness 通用)

L.E.E.K 的 user 是**产品负责人**,不是 code reviewer。Ta 决定要做什么 / 优先级 / 产品形态;agent 决定怎么做(工程实现)。沟通必须按这个角色边界来,**否则信息超载,用户失去对项目方向感的掌控**。

### 默认隐藏 — 不要主动发给用户

以下内容**只在用户明问时才给**:

- commit hash / branch 状态 / push 与否
- LOC / 改了几个文件 / 哪些文件
- `cargo test` 数字 / clippy warning 数
- 内部 Rust 类型名 / 模块路径 / 字段名 / API endpoint 路径
- subagent ID / worktree path / cherry-pick / merge conflict 解决细节
- 工程取舍的术语级表达("用 trait per shape 而非 trait per vendor")

### 默认要给 — 用户视角的功能描述

每个 milestone 完成 / 每次状态汇报,必带:

- **能用什么了**(用户视角,不是 milestone 名):"以后 leek 不会卡在反复读同一份 PDF 上几分钟不出声了" ✅
- **没做到什么**(用户能感知的 gap):"reasoning 阶段还是没'agent 思考中'指示器" ✅
- **需要拍什么板**(只问产品/UX 决策,不问技术):"A 股 deep-review 5-15 分钟正常,你能接受多久才该弹'不耐烦'提示?" ✅

### 错误示范 vs 正确示范

| ❌ 错(纯术语)| ✅ 对(用户视角)|
|---|---|
| `cargo test 313 passed, M3.1 cherry-pick 51f07b3 push 到 origin` | `你刚踩的"PDF 重复 9 次"坑修了 — codex 重复访问同一页到第 3 次屏幕弹警告,到第 7 次直接停 turn,阈值你能在 Settings 调` |
| `要 spawn subagent 改 estimator,还是用 max(last_input, current_estimate)?` | `compaction 那个 bug 你倾向"宁可早误触发,不要漏触发"对吧?` |
| `polish dispatch:reasoning status pill / max_iterations reset` | `修一下你刚踩到的"agent 4 分钟不出声让你以为卡死"的体验问题` |

### 用户 opt-in 展开技术细节

用户说**任一个**:`展开` / `细节` / `具体怎么改的` / `代码层面` / `给我看 commit` → 这时才可以下到 LOC / commit hash / 模块路径那一层。

用户主动用术语对答(例:"那个 estimator 准吗"),说明 ta 想下到那个层级 → 可以技术对答。

### 决策框架

要用户拍板时,把选项框成**用户语言的 trade-off**,2-4 个,**每个一行**:

> **A** 修你刚踩的"agent 静默 4 分钟没反馈"那个体验问题
> **B** 继续推新功能(A 股 deep-review 更深)
> **C** 你浏览器实测 T1-T34 一遍,看还有什么 bug

不要把决策框成 `enabling tiktoken-rs / falling back to char ratio` 这种。

### 翻译原则

每个工程动作 / dispatch / commit,在汇报给用户时**至少加一句 plain language 翻译**(可以是一句话总结整段技术内容):"对你来说意味着 X"。

如果你写完汇报发现 ta 一段话里全是术语没翻译,**重写**。

### 例外:用户在 plan / dispatch / docs 阶段

用户在写 dispatch md / planning 阶段,主动要看 spec 细节(因为 ta 要 dispatch worker 或决定 scope),这时**给技术细节是对的** — 这是 user 临时切到 dev 视角。会话回到日常使用 / 测试场景,自动切回用户视角。

判断信号:
- 用户问"我怎么测这个" / "现在怎么样" / "为什么这样" → **用户视角**
- 用户问"dispatch 写了什么" / "spec 怎么定的" / "改了哪几行" → **临时 dev 视角**

---

## Working conventions

- Current product, UX, architecture, and milestone artifacts go under `docs/`.
- The existing `design/` tree is historical archive material. Do not add new authoritative specs there.
- Scratch / temporary files go under `tmp/` (gitignored).
- **Browser/Playwright 截图必须保存到 `tmp/`，严禁落到项目根目录。** 测试结束后 `tmp/` 内容可直接清空，不需逐一确认。

## Authoritative documents

Authority order for L.E.E.K itself:

1. Current code.
2. [`docs/REQUIREMENTS.md`](docs/REQUIREMENTS.md) for product / UX / acceptance requirements.
3. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for architecture.
4. [`docs/MILESTONES.md`](docs/MILESTONES.md) for phase order and completion state.
5. `design/` only as non-authoritative historical reference.

The corpus has its own authority (`finance-giant/corpus/AGENTS.md`) — when the two disagree about *the corpus*, that file wins; about *the agent*, this project wins.
