# L.E.E.K Frontend Concept

> chat-canvas 形态、产品愿景、用户旅程、视觉与交互原则。本文档是给 UX 设计师的入口文档——读完它应该能 cold-start 出 wireframe 和视觉稿。

## 1. 产品定位

**L.E.E.K (老韭菜) 是一个投研操作系统**，给希望停止做"市场韭菜"的散户用。它把一份策划过的投资智慧（corpus）变成可执行的研究、决策与复盘。

### 核心叙事：用户是 manager，L.E.E.K 是研究团队

L.E.E.K 对用户来说**不是一个 chat 工具**，更像他指挥的**研究团队**：

- 用户作为基金经理（manager），**给团队下达任务**——不是问问题，是布置工作
- 团队（main agent + 临时调度的 strike teams）执行任务，过程实时可见
- 团队产出 **deliverable**（决策草稿 / 调研简报 / 复盘 / 对比报告）
- 用户 review、编辑、最终 **confirm 或 reject**

这个定位带来的关键产品语义：

- **任务（task）是后端的核心交互单元**——但在前端**隐式**，用户感知到的是 chat 主轴 + canvas DAG
- **主输入是 chat 主轴的自然语言输入框**——agent 从 user message 自动提取 task.goal / constraints / expected_deliverable
- **Agent 工作过程在 canvas DAG 上展开**——chat 主轴只放 user input 与 agent 的最终简短回复，typed deliverable 节点是仪式性产出
- **用户 ownership = 决策由我做出，团队是我的研究资源**

它**不是**：
- 不是聊天机器人（chat thread 只是 task 内追问的形态）
- 不是 dashboard（数据是为思考服务的，不是反过来）
- 不是模拟交易系统（它输出决策草稿，不下单）
- 不是一个 turn-key 的"投资 Buddy AI"——它是**你自己的投资 OS**，由你自己策划的智慧驱动

详细的 manager + team 交互模型见 [`../interaction-model.md`](../interaction-model.md)。

## 2. 视觉冲击力的核心叙事

L.E.E.K 的产品差异化不在"另一个聊天框 + 另一个图表"，而在**两个 striking 的视觉时刻**：

### 2.1 Corpus 大脑——思考时神经元激活 ⭐

L.E.E.K 的 corpus 是手工策划的投资智慧库（数百篇概念页 + entity 档案 + 原材料），节点之间用 wikilink 连接成一张知识图谱。

**视觉冲击力的核心**：当 agent 在分析中检索 / 引用 corpus 中的某个概念时，**对应的"神经元"在大脑视图里被激活**——节点脉冲、颜色变化、边缘扩散动效，像真正的大脑神经元被点亮。

```
                  ┌─────────────────────────────────────┐
                  │ 大脑视图（Corpus Brain）             │
                  │                                     │
                  │       ●────●────● 激活脉冲          │
                  │       │   ╱│   ╲                    │
                  │       ●──● │    ●                   │
                  │      ╱  ╲ ●────●                    │
                  │     ●    ●     │                    │
                  │     │   ╱  ╲   ●                    │
                  │     ●──●    ●                       │
                  │            ╲                        │
                  │       ●     ●  ●                    │
                  │                                     │
                  └─────────────────────────────────────┘
                            ↑
            激活强度 3 级：
            · search hit  → 节点浅色脉冲一次
            · deep read    → 节点强色 + 1s 持续
            · cited in decision → 节点变核心色 + 边缘 ripple
```

**这个视觉是 L.E.E.K 的 signature moment**：

- 它把"AI 思考"这个抽象概念变成可见的过程
- 它直接展示用户**自己策划的智慧库**正在被使用——比"我的 AI 帮我答了一个问题"更有 ownership
- 它激励用户继续维护 corpus（节点越多 / 连接越密 / 激活路径越深 = 思考能力越强）

UX 设计师对这个 panel 应该投入最多 craft——它是产品的灵魂场景。

### 2.2 思维链 DAG with 动效

agent 当前任务的推理过程实时展开成一张 DAG：

- 起点节点：用户的问题
- 中间节点：每个推理步骤 / 工具调用 / 观察
- 终点：决策草稿 / 答复
- 当前活跃节点：脉冲高亮
- 边连接：随推理流出现，带 traveling pulse 动效

ReasoningDAG 与 CorpusBrain **可联动**：点击 ReasoningDAG 中的"我用了 margin-of-safety 这个原则"节点，CorpusBrain 同步高亮对应的 corpus 概念页节点。

这两个 panel 是 L.E.E.K 的**视觉语言核心**。其他 panel 围绕它们组织。

## 3. 整体形态：chat-canvas

L.E.E.K 的主界面是 **chat-canvas**——一个 chat 主轴 + 一个动态 canvas 的混合形态：

```
┌────────────────────────────────────────────────────────────────────────┐
│ L.E.E.K   [Sessions] [Vault] [Charter]               [Settings] [User]│
├──────────────────┬──────────────────────────────────┬──────────────────┤
│ Chat 主轴 (左)    │  Canvas = Reasoning DAG (主区域)  │ CorpusBrain      │
│                  │                                  │ (右栏 ambient)   │
│ TaskBar:         │  ◆ user: NVDA 加仓?              │                  │
│ ⟳ in_progress    │   │                              │   ●──●──●        │
│ 评估 NVDA 加仓    │   ●─→ ToolCall: portfolio        │   神经元激活中   │
│ [追加约束][中断] │   │   ▼                          │   margin-of-     │
│                  │   ● Observation: 已有 50 股      │   safety...      │
│ ─────────────── │   │                              │                  │
│                  │   ●─→ ToolCall: quote NVDA       │ ─────────────── │
│ 你: NVDA 加仓?   │   │   ▼                          │ Watchlist 摘要   │
│ 我已经有 50 股    │   ┌──────────────┐               │  NVDA  ↑ 2.1%   │
│                  │   │Quote 节点     │               │  GOOGL ↑ 1.2%   │
│ L.E.E.K: 完整    │   │$480.20 ↑2.1% │               │ ─────────────── │
│ 决策草稿在画布   │   └──────────────┘               │ Portfolio 摘要   │
│ 上 →             │   │                              │  Total $124.8K   │
│                  │   ┌──────────────────────────┐  │  Tech 75% ⚠      │
│                  │   │📌 Subagent: valuation_dcf│  │                  │
│ ──────────────  │   │  turn 1/2/3 ✓            │  │                  │
│ [输入框]         │   │  result: DCF $520        │  │                  │
│ ▶ 追问 / 下任务  │   └──────────────────────────┘  │                  │
│                  │   │                              │                  │
│                  │   🧠 corpus: margin-of-safety    │                  │
│                  │   │                              │                  │
│                  │   ┌──────────────────────────┐  │                  │
│                  │   │DecisionDraft 节点         │  │                  │
│                  │   │ long +15 股 stop $440     │  │                  │
│                  │   │ ✓ size 1.5% < 10%         │  │                  │
│                  │   │ ⚠️ 集中度警告              │  │                  │
│                  │   │[Confirm][Discuss][Reject] │  │                  │
│                  │   └──────────────────────────┘  │                  │
└──────────────────┴──────────────────────────────────┴──────────────────┘
```

### 关键设计原则

1. **Canvas 是一棵活的 Reasoning DAG**——不是 dashboard 风格的"多个独立 panel 摆放"，而是一棵流动的、可视化的思维树。每种 panel（Quote / Chart / DecisionDraft / ...）都是 DAG 上的一个 typed 节点，不是独立窗格
2. **Chat 主轴极简**——只显示 user message 和 agent 的最终回复（含"完整结果在画布上 →"指引），**不在 chat 里塞中间过程**。中间所有 thinking / tool call / observation / subagent / corpus 引用都在 canvas DAG 上展开
3. **TaskBar 是 chat 主轴顶部的 slim bar**——显示当前 task status + 干预按钮（追加约束 / 中断），**不让 task 概念变重**。没有 task creator form，task 由 agent 从 user message 隐式提取
4. **Subagent 在 DAG 中显式可见**——📌 容器节点封装内部 turn 序列，用户能看到"团队派遣了估值小组"而不是黑盒
5. **CorpusBrain 是右栏 ambient 视图**——常驻侧栏，agent 检索 / 引用 corpus 时神经元激活脉冲，与 canvas DAG 中的 corpus_ref 节点联动高亮
6. **Deliverable 节点是仪式性产出**——Confirm / Discuss / Reject 是有视觉重感的动作，不是随手 like
7. **节点位置由 DAG 布局算法决定**——用户不能拖动 / 钉住 / 关闭节点；只能 zoom / 折叠分支 / 全屏某节点
8. **历史可重放**——task 完成后整棵 DAG 持久化，可重放动效
9. **没有"agent 在打字"的 placeholder**——thinking 直接以 DAG 中的 thinking 节点流式逐字展开，不是"..."等待动画

## 4. 用户旅程

### 4.1 典型 reactive 任务（用户主动下达）

```
Step 1  用户打开 leek (localhost:8964)
        - 看到三栏布局：
          · 左：chat 主轴（含 TaskBar / 输入框）
          · 中：canvas（空 / 上次 task 的 DAG）
          · 右：CorpusBrain ambient + Watchlist + Portfolio 摘要

Step 2  用户在 chat 输入框输入：
        "NVDA 现在能加仓吗？我已经有 50 股"
        Cmd+Enter → 提交

Step 3  系统隐式创建 task（用户感知不到 form / 状态机）
        - main agent 收到 user message → 自动从中提取 task.goal / constraints
        - TaskBar 显示 ⟳ in_progress · 评估 NVDA 加仓 · 0s
        - canvas 长出第一个节点：◆ user_input
        - main agent 进入工作循环

Step 4  Canvas DAG 实时展开（用户看到团队在工作）
        - 节点逐个出现，边逐条连接：
          ◆ user_input
            ↓
          ●→ tool_call: vault.holdings.current
            ↓
          ●  observation: 已有 50 股 (1.7%)
            ↓
          ●→ tool_call: quote NVDA
            ↓
          [Quote 节点]: NVDA $480.20 ↑ 2.1%
            ↓
          📌 subagent_branch: valuation_dcf
              (内部 turn 1/2/3 进度可见，可折叠)
            ↓
          🧠 corpus_ref: margin-of-safety  ← 同时 CorpusBrain 节点激活
          🧠 corpus_ref: owners-earnings
            ↓
          ● thinking: 估值偏高，但护城河...
            ↓
          ◆ decision_draft 节点
        - chat 主轴此时只有用户的原话，没有"agent 正在思考..."这种填充
        - TaskBar 实时更新：⟳ in_progress · turn 3/5 · 24s

Step 5  Agent 输出 deliverable，写到 chat
        - DecisionDraft 节点出现在 canvas 末端（含完整 form：
          ticker / direction / size / stop / horizon / rationale /
          corpus_refs / mandate_check）
        - TaskBar → ✓ delivered
        - chat 主轴出现 agent 简短回复：
          "L.E.E.K: 建议加仓 15 股，止损 $440，期限 120 天。
                   完整决策草稿已生成在画布上 →"

Step 6  用户 review deliverable（仪式性动作）
        - 用户点画布上的 DecisionDraft 节点 → 进入 fullscreen 模式
        - 行内编辑任何字段（编辑时 mandate_check 实时重算）
        - 三个 action 按钮（视觉上有重感）：
            [ ✓ Confirm ]  [ ⟲ Discuss ]  [ ✕ Reject ]
        - Confirm → task → confirmed，decision 写入 vault.decisions
        - Discuss → 用户在 chat 框追问，main agent 调整 deliverable
        - Reject → task → rejected (留理由)

Step 7  用户在真实账户下单后回到 leek
        - 主动更新 Portfolio 摘要：NVDA 50 → 65 股
        - 后续 task 中 agent 看到新持仓
```

### 4.2 中途干预（manager 风格）

```
任务 in_progress 中，用户在 TaskBar 看到：
  ⟳ in_progress · 评估 NVDA 加仓 · 24s
  Agent 当前: corpus.search "margin of safety"
  [追加约束] [中断]

场景 A: 追加约束
  用户点 [追加约束] → 弹一个小输入框，输入"也考虑一下 BABA 的对比"
  → 这是 control 命令（不进 chat 主轴的 message stream）
  → 注入 main agent context，下一轮 LLM call 看到，自动调整行动

场景 B: 自然语言追问（也可以追加约束）
  用户在 chat 输入框直接写"也考虑一下 BABA 的对比"
  → 进入 chat thread，agent 智能识别为约束（vs 全新问题）
  → 同样 inject 到当前 task

场景 C: 中断
  用户点 [中断] → task → cancelled
  → canvas DAG 保留所有已产出的节点
  → 用户可从 cancelled 状态 fork 新 task（带 context）
```

### 4.3 Proactive 模式（系统创建任务）

```
Step 1  Cron 触发：3 个月前的 NVDA decision 到 review 期
        - 系统在后台创建 task（status=queued, source=cron, expected_deliverable=review）
        - 用户下次打开 leek 时 chat 主轴顶部 TaskBar 显示：
          "🔔 1 个待复盘任务"  → [立即开始] [延期]
        - 用户点击 [立即开始] → task 进 in_progress

Step 2  Canvas 长出复盘 DAG
        - main agent 自动准备复盘 context：
          - 拉当时的 decision 完整内容
          - 拉决策时点 vs 当前的 portfolio 对比
          - 拉 NVDA 这 3 个月的行情走势
          - 拉决策引用过的 corpus 节点（CorpusBrain 高亮回放激活路径）
        - DAG: ◆ task_start → tool_call (decision) → tool_call (portfolio diff)
               → tool_call (chart) → 🧠 corpus_ref × N → thinking → ◆ review_draft

Step 3  Review 节点出现在 canvas 末端
        - 含评分（self_score / agent_score）+ lessons_md（可编辑）
          + corpus_inbox_candidates checkbox
        - chat 主轴：agent "复盘草稿已生成 →"
        - 用户编辑 → Confirm → review 写入 vault.reviews
```

### 4.4 Browse 模式（无目的探索）

```
Step 1  用户打开 leek 但不输入任何内容
        - canvas 是空的（或上次 task 的 DAG 折叠展示）
        - chat 主轴 TaskBar 显示 "idle · 提个问题或下达任务，团队待命"

Step 2  CorpusBrain 在右栏缓慢自转 / 微脉冲（暗示"在等"）

Step 3  用户点 CorpusBrain → 全屏接管主区域，进入大脑全景视图
        - 拖动 / 缩放 / 搜索
        - 点击节点 → 弹出 corpus 概念页 popover
        - 看节点的"近期激活历史"

Step 4  用户被某个概念吸引（如 "owners-earnings"）
        - 弹出 Article popover 显示 corpus 内容
        - 旁边推荐："基于这个概念，给团队起一个分析任务?"
        - 用户点击 → chat 输入框聚焦并预填 mention chip `@corpus:owners-earnings`，
          光标停在后面让用户接着写自然语言（如"用这个框架看看 BABA"）
```

## 5. 视觉与交互原则

### 5.1 视觉

- **暗色主题为默认**（金融工作场景，长时间盯屏，护眼）；亮色主题作为可选
- **数据 first**：数字 / 图表 / 节点是主角；装饰性视觉元素（背景、icon 风格、阴影）极简
- **彩色保留给"信号"**：上涨绿 / 下跌红 / 激活金黄 / 警告橙 / 错误红——其他 UI 都是灰阶
- **字体**：等宽（金融数字用 SF Mono / JetBrains Mono / IBM Plex Mono）+ 正文 sans-serif
- **空间**：呼吸感优先于"信息密度最大化"——L.E.E.K 不是 Bloomberg dashboard
- **动效**：所有动效服务于"理解发生了什么"——节点出现 / 边连接 / 数据更新都有微动效，但不超过 300ms
- **视觉层次**：Canvas DAG 是核心舞台（agent 工作过程），CorpusBrain 是右栏 ambient 二号舞台

### 5.2 交互

- **键盘优先**：Ctrl+K 打开 command palette、`/` 聚焦输入框、`@` 触发 mention（@ticker / @decision / @corpus-concept）
- **节点位置由 DAG 布局算法决定**——用户不能拖动 / 钉住 / 关闭节点
- **节点的用户控制**：滚轮缩放 / 空白拖动平移 / 折叠子分支 / 点击进入 fullscreen
- **mention chip**：chat 输入框 `@NVDA` 触发 ticker chip，`@corpus:margin-of-safety` 触发 corpus chip——这些 chip 会被 agent 作为强 context
- **拖拽**：从 Watchlist / Portfolio 拖一个 ticker 到 chat 输入框 = 自动插入 mention chip
- **激活联动**：点击 canvas 中 corpus_ref 节点 → CorpusBrain 同步高亮关联节点；反之亦然

### 5.3 实时性的视觉表达

- **chat 主轴极简**：流式接收 LLM 输出 → chat 主轴文字逐字 / 逐 token 出现，**只显示最终回复**
- **canvas DAG 是工作过程的可视化**：节点逐个出现，边逐条连接，**当前活跃节点持续脉冲**直到下一节点接力
- **thinking 节点流式逐字**：用户看到 agent 在"想"什么
- **高频 quote 数字**：textContent 直接更新 + 最近一次变化方向用极淡的 flash（绿/红 100ms 渐隐）
- **CorpusBrain 激活**：节点脉冲峰值 ~150ms，扩散 ripple 持续 ~500ms

## 6. 三种使用模式

L.E.E.K **不假设**用户"知道自己想问什么"。三种模式都是一等公民：

### Reactive（用户主动下任务）
- 用户在 chat 输入框输入自然语言 → agent 隐式提取 task → 在 canvas 上工作 → 输出 deliverable 节点
- 80%+ 使用场景走这条
- 不需要 user 填 form。Task 是后端的实施细节，前端 UI 不暴露 task 概念

### Proactive（系统创建任务给用户）
- Cron 触发的 review reminder（系统创建 task）
- 用户打开 leek 时 TaskBar 顶部提示"🔔 N 个待处理"
- 用户**显式接受**才进入 in_progress——agent 不会越过用户自动开始
- **不做**：daily briefing 主动推送（推迟）

### Browse（无目的探索）
- 进 CorpusBrain 全屏全景视图自己浏览
- 翻历史 sessions / decisions / reviews
- Watchlist / Portfolio 摘要查看
- 浏览中产生兴趣 → 一句话发到 chat 框启动新任务

## 7. 用户身份与 Team Charter

每个用户启动 leek 时，gateway 接受 user_id（默认 OS 用户名）。所有 vault 数据按 user_id 隔离。

用户的**Team Charter**（升级版的 mandate）是 L.E.E.K 的硬约束 + 软偏好：

```yaml
# 风格
style: [long-term-fundamental, margin-of-safety, ...]

# 硬约束（agent 不能逾越）
hard_limits:
  max_position_pct: 10
  forbidden_instruments: [options, leveraged_etfs]

# 软偏好（agent 可以建议但要 flag）
soft_preferences:
  preferred_sectors: [tech, consumer-staples]
  avoid_sectors: [tobacco]

# 工作风格
work_style:
  decision_verbosity: detailed
  cite_corpus_always: true
  challenge_my_bias: true
```

- 用户在 Charter 编辑器（可视化 form）维护
- agent 每次 task 都把 Charter 注入 system prompt
- agent 输出 deliverable 时**必须**做 mandate check 并显式列出 violations
- 详见 [`../interaction-model.md`](../interaction-model.md) §6

Charter 的可视化是产品的另一个温柔的 detail——让用户感到"我在指挥团队，团队按我的章程工作"。

## 8. 边界与不做的事

- ❌ **下单**：leek 不接券商，不下单。决策草稿用户在真实账户里执行
- ❌ **PnL / 收益曲线**：没有 paper trading（[ADR-0008](../decisions/0008-no-paper-trading.md)）
- ❌ **股价预测自信值**：agent 给推理 + 引用，**不给概率值 / 置信度**——没有保护用户免责
- ❌ **协作 / 多人共享**：P1 单用户，不做 share / comment / 协作
- ❌ **移动 native app**：P3+，P1 是 web

## 9. 设计参考

| 来源 | 借鉴 | 不借鉴 |
|--|--|--|
| **ChatGPT** | chat 主轴形态、流式输出 | 单窗格、缺乏动态 panel |
| **Warp Terminal** | typed artifacts (Plan/PR/File)、blocks 模型、context chips、workflow lock 输入 | 闭源、AGPL-3.0、终端为根（我们是 web） |
| **Bloomberg Terminal** | 多 panel 工作台、行情数据密度、键盘优先 | 70 年代视觉风格、信息过载、付费高墙 |
| **TradingView** | K 线图视觉品质、Canvas-based 性能 | 主要面向 day trader、UI 偏交易 |
| **Obsidian Graph View** | **Corpus Brain 直接视觉来源**：节点 / 边 / 力导布局 | Obsidian 的图谱是静态浏览，缺乏激活动效 |
| **MIRO / FigJam** | 自由摆放面板的 canvas 心智模型 | 协作工具，我们 P1 单用户 |
| **Linear** | 极简暗色金融感、键盘 shortcut 文化 | 项目管理 UI 模式不直接套用 |

## 10. 给 UX 设计师的关键约束

1. **核心心智：Canvas = Reasoning DAG**——主区域不是 dashboard 风格的多窗格摆放，是一棵活的、流动的思维树。每种 panel（Quote / Chart / DecisionDraft / ...）都是 DAG 上的 typed 节点，不是独立窗格
2. **Chat 主轴极简**——只显示用户输入和 agent 的最终回复，**不在 chat 里塞中间过程**（中间过程都在 canvas DAG 上）
3. **Task 概念在前端是隐式的**——没有 Task Creator form，用户输入自然语言，agent 自己提取 task。TaskBar 仅作为顶部 slim 状态条 + 干预按钮
4. **CorpusBrain 与 Canvas DAG 联动**——Canvas 中的 corpus_ref 节点点击 ↔ CorpusBrain 节点高亮；CorpusBrain 是右栏 ambient 二号舞台，花最多 craft 在这里
5. **Subagent 在 Canvas DAG 中显式可见**——用户看到 📌 容器节点（"派遣了估值小组"），点击展开内部 turn 序列。但不能直接和 subagent 对话（subagent 是 lead 的内部资源）
6. **干预区是 TaskBar 上的 slim button**——[追加约束] [中断] 在 chat 主轴顶端 first-class，不藏在右键菜单
7. **Deliverable confirm/reject 是仪式性动作**——decision_draft / review_draft 节点上的 Confirm 按钮要有"我做了决策"的视觉重感
8. **Team Charter 编辑器**——用户表达自己的入口，做得好会让用户感到 ownership
9. **流式动效不超过 300ms**——再长就阻塞用户感知
10. **节点不能拖动 / 钉住 / 关闭**——位置由 DAG 布局算法决定，避免 dashboard 风格的窗格管理
11. **数据 first，装饰极简**——不要 dashboard 风格的繁复 widget；金融人对花哨视觉敏感（觉得"不专业"）
12. **暗色为默认**——金融工作场景常态
13. **键盘优先**——目标用户是有耐心 / 注重效率的散户，键盘文化欢迎

## 11. 后续依赖文档

- [`../interaction-model.md`](../interaction-model.md) —— Manager + Team 交互模型完整定义
- [`panels.md`](panels.md) —— 完整 panel 清单 + 每类 panel 的详细设计
- `tech-stack.md`（待写） —— 技术栈（SolidJS / Tailwind / Kobalte / Canvas 库选型）
- [`../p1-spec/api.md`](../p1-spec/api.md) —— 前端要消费的事件协议
- [`../p1-spec/agent-loop.md`](../p1-spec/agent-loop.md) —— agent 的 phase 模型决定 panel 的状态机
- [`../p1-spec/llm-provider.md`](../p1-spec/llm-provider.md) §10 —— Provider 配置 UI 形态描述
