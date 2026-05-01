# L.E.E.K Interaction Model

> 用户与 L.E.E.K 协作的核心模型——产品形态层面的"行为契约"。本文档与 [`architecture.md`](architecture.md) 同级，是另一份 root-level 设计文档：架构 = 系统怎么搭，interaction-model = 用户怎么和它工作。

读完本文档，你应该明白：
- 用户在 L.E.E.K 里扮演的角色是什么
- 什么是"任务"，它从哪里来、走过哪些状态、最终变成什么
- 用户在每个阶段可以做什么、不能做什么
- 这一切为什么不像 ChatGPT

## 1. 核心定位：用户是 manager，不是客户

L.E.E.K 不是"投资顾问 chatbot"。它的核心定位是：

> 用户是一个独立的投资经理（自己的资金 / 自己的判断 / 自己的责任），L.E.E.K 是他可以指挥的研究团队。

这个定位带来的语义改变：

| 维度 | ChatGPT 模式 | L.E.E.K Manager 模式 |
|--|--|--|
| 用户角色 | 客户 / 提问者 | **基金经理 / 决策者** |
| Agent 角色 | 知识助手 | **下属团队（lead + 临时调度的 strike teams）** |
| 交互单元 | message / turn | **任务（task）** |
| 输入主形态 | "回答这个问题" | **"完成这个目标，按这些约束"** |
| Agent 输出形态 | reply 文字 | **deliverable（决策草稿 / 调研报告 / 复盘 / 简报）** |
| 用户参与度 | 偶尔追问 / 等结果 | **持续跟进 / 干预 / 重 scope / 重指派** |
| 责任归属 | "AI 给的建议" | **"我的研究团队提交的草稿，由我决策"** |

L.E.E.K 设计上**强化用户的 ownership**——不是让用户感到"AI 替我做了"，而是让用户感到"我的团队在为我工作，我在做最终决策"。

## 2. 三层角色

```
┌────────────────────────────────────────────────────────────┐
│                                                            │
│   User (Manager)                                           │
│     · 下达任务                                              │
│     · 设定 mandate（team charter）                         │
│     · review / confirm deliverable                         │
│     · 追加约束 / 重 scope / 中断                           │
│                                                            │
│        ↓ task                  ↑ deliverable               │
│                                                            │
│   Main Agent (Lead / Coordinator)                          │
│     · 持有 session 主上下文                                 │
│     · 接收任务，规划执行步骤                                │
│     · 直接调用工具（行情 / 资讯 / corpus / vault / ...）   │
│     · 当某子任务需要"clean room"或并行时，spawn subagent  │
│     · 收集所有 observation，撰写 deliverable               │
│     · 实时向用户汇报进度（reasoning DAG / panel）          │
│                                                            │
│        ↓ spawn (with scope)     ↑ structured return        │
│                                                            │
│   Subagent (Strike Team, on-demand)                        │
│     · 短暂生命周期，做完一件事就消失                         │
│     · 独立 context，不污染主 agent                          │
│     · 拿到主 agent 给的明确 scope + 工具子集                │
│     · 返回 structured result（不是流式聊天）                │
│     · 用户**不直接管理 subagent**——它们是 lead 的内部资源   │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

### 三个关键不变性

1. **用户只与 Main Agent 对话**——不与 subagent 直接交互。subagent 是 lead 的实现细节，对用户透明（但其工作过程可视化）。
2. **Main Agent 持有所有持久化状态**——决策、复盘、portfolio 同步、mandate check 都在主 agent 这一层执行。subagent 不写 vault。
3. **Subagent 是 map-reduce 风格**——spawn 时给明确 scope（"用 X 工具查 Y，返回 Z 字段"），返回 structured value，主 agent 决定怎么 merge。**不是常驻 specialist**。

## 3. 什么是"任务"

任务（Task）是 L.E.E.K 的核心交互单元。它**取代了 ChatGPT 的 message/turn**——所有用户的目标都先落成任务。

### 3.1 任务的形态

一个任务包含：

```typescript
type Task = {
  id: string;
  user_id: string;
  session_id: string;             // 任务总属于某个 session（chat 主轴线程）
  
  // 核心字段
  title: string;                  // "评估 NVDA 加仓" / "本周 portfolio 复盘"
  goal: string;                   // 自然语言描述（agent 主要从这理解意图）
  
  // 约束（manager 给的边界）
  constraints: {
    scope?: string;               // "只看美股" / "只用 corpus 现有信息"
    horizon?: string;             // "短期（1 周）" / "长期（半年以上）"
    risk_budget?: string;         // "最大暴露 2% 仓位"
    avoid?: string[];             // "不要看小盘股"
    deadline?: string;            // ISO date or "EOD" / "tomorrow morning"
  };
  
  // 期望产出
  expected_deliverable: 
    | "decision_draft"            // 决策草稿（含仓位 / 止损 / 期限）
    | "research_brief"            // 调研简报（不含决策）
    | "review"                    // 复盘
    | "comparison"                // 多标的对比
    | "morning_brief"             // 晨报式摘要
    | "free_form";                // 让 agent 自己决定
  
  // 状态
  status: 
    | "draft"                     // 用户还在编辑任务卡
    | "queued"                    // 已提交，等 agent 接
    | "in_progress"               // agent 在做
    | "awaiting_user"             // 需要用户输入才能继续（如澄清问题）
    | "delivered"                 // 已交付 deliverable，等 user review
    | "confirmed"                 // user accepted
    | "rejected"                  // user rejected → 可能 respawn 一个新任务
    | "cancelled"                 // 用户中断
    | "failed";                   // agent 执行错误
  
  priority: "low" | "normal" | "high" | "urgent";
  created_at: string;
  updated_at: string;
  closed_at?: string;
};
```

### 3.2 任务从哪里来

**重要**：Task 是后端实施细节，**前端 UI 不暴露 task 概念**——用户在 chat 输入自然语言，main agent 自己提取 task。详见 §4 输入形态。

任务的来源（系统视角）：

1. **从 user message 隐式提取**（最常见）：用户在 chat 主轴输入"NVDA 现在能加仓吗？我已经有 50 股" → main agent 自动构造 task（goal / constraints / expected_deliverable 由 agent 推断）
2. **Cron 触发**：mandate 设了"每周一晨报"或某 decision 到 review 期 → 系统自动创建任务（status=queued），等用户打开 leek 时 TaskBar 顶部提示
3. **Agent 提议升级**：在已有 task thread 中用户问了一个新方向的问题，agent 判断"这是新 task"，提议另开 → 用户确认或合并到当前

无论哪种来源，**所有任务都进同一个后端 task lifecycle**——便于追溯、复盘、审计。但用户感知层面：他只是在 chat 里说话。

### 3.3 任务的生命周期

```
                 ┌─────────────────────────────────┐
                 │ User creates task card          │
                 │  (title + goal + constraints)   │
                 └────────────┬────────────────────┘
                              │ submit
                              ▼
                          ┌───────┐
                          │queued │
                          └───┬───┘
                              │ agent picks up
                              ▼
            ┌─────────────────────────────────────┐
            │       in_progress                   │
            │                                     │
            │   Main Agent loop:                  │
            │     · 拼 context（含 task / mandate）│
            │     · 决定下一步行动                  │
            │     · 调工具 / spawn subagent        │
            │     · 观察结果，迭代                  │
            │                                     │
            │   实时向用户推：                     │
            │     · reasoning DAG 节点             │
            │     · tool call 进度                 │
            │     · corpus brain 激活              │
            │     · panel 召唤                     │
            └────────────┬────────────────────────┘
                         │
            ┌────────────┼────────────┬─────────────────┬───────────────┐
            ▼            ▼            ▼                 ▼               ▼
       ┌──────────┐ ┌─────────┐ ┌───────────┐     ┌──────────┐    ┌────────┐
       │delivered │ │awaiting │ │  failed   │     │cancelled │    │  ...   │
       │          │ │  user   │ │           │     │          │    │        │
       └────┬─────┘ └────┬────┘ └───────────┘     └──────────┘    └────────┘
            │            │
            │ user        │ user 答 → 回 in_progress
            │ reviews     │
            ▼            
       ┌──────────────────┐
       │ confirmed │       
       │ rejected  │       
       └──────────────────┘
```

### 3.4 用户在每个阶段可以做的事

| 状态 | 用户能做什么 |
|--|--|
| `draft` | 编辑任意字段；保存为 draft 不提交；删除 |
| `queued` | 取消；改优先级；编辑 constraints |
| `in_progress` | **追加约束**（"也考虑一下 BABA 的对比"）；**重 scope**（"换成只看一年期的"）；**中断**；**钉住某个 panel 让它继续**；**追问澄清** |
| `awaiting_user` | 必须答 agent 的问题才能继续；或显式取消 |
| `delivered` | review deliverable；编辑某些字段（如 decision draft 的止损）；**confirm** / **reject** / **respawn 新任务**（基于这个 deliverable 起新任务） |
| `confirmed` / `rejected` | 任务 sealed。但 deliverable 进 vault 永久保留 |
| `cancelled` / `failed` | 可以从这里 fork 一个新任务（带原任务的 context）|

**关键交互能力**：

- **追加约束** = 不打断当前 agent 工作，把新约束插入 main context，agent 下一步迭代会看到
- **重 scope** = 中断当前工作，重置 task 的 constraints / goal，agent 重新规划
- **中断** = 立即停（不强制 agent 立即收尾，但下个 LLM call 之前会被打断）
- **追问澄清** = 用户在 task thread 里发普通 message，不改任务字段

## 4. 输入形态：单一 chat 主轴

L.E.E.K 的主输入是 **chat 主轴的输入框**——单一形态，所有用户表达都从这里进。

### 4.1 主输入：chat 框

```
┌─────────────────────────────────────────────────────────────┐
│  你: NVDA 现在能加仓吗？我已经有 50 股                        │
│                                                             │
│  L.E.E.K: 建议加仓 15 股，止损 $440，期限 120 天。           │
│           完整决策草稿已生成在画布上 →                       │
├─────────────────────────────────────────────────────────────┤
│ [输入框]                                                    │
│  ▶ 给团队下达任务、追问、或提建议...        [Send ↵]        │
│                                                             │
│  支持 mention chip：                                         │
│  @NVDA  @portfolio:current  @corpus:margin-of-safety        │
│  @decision:abc  @task:xyz                                   │
└─────────────────────────────────────────────────────────────┘
```

- 自然语言输入——**不是结构化 form**
- Cmd+Enter 发送
- 支持 mention chip（typed reference 注入 agent context）
- 多行支持（Shift+Enter 换行）
- agent 从输入中**自动提取**：goal / constraints / expected_deliverable / context_refs

### 4.2 Agent 如何从自然语言推 task

main agent 的第一轮 LLM call 会读 user message + 最近若干历史 → 决定：

- **新 task**：用户表达了一个明确目标或问题
- **追加到当前 task**：用户在已有 in_progress task thread 内补充约束 / 改 scope
- **闲聊 / 浏览意图**：不开 task，直接简短回复（如"你好"、"今天怎么样"）

agent 的判断是 fuzzy 的，但用户不需要管——他只是说话。

### 4.3 中途追加约束的两种方式

**方式 A（自然语言）**：在 chat 输入框直接说"也考虑一下 BABA"
- agent 智能识别为约束 vs 全新方向（如果 ambiguous，agent 会反问"我把它当作当前任务的追加约束，还是另开一个任务？"）

**方式 B（control 命令，TaskBar 上）**：点 [追加约束] 弹小输入框
- 显式标记为 control 而非 chat message
- 不进 chat 主轴的 message stream（保持 chat 主轴干净）
- 直接 inject 到 main agent context

### 4.4 用户感知不到的形态

用户**不会看到**：
- ❌ Task creation form / Title-Goal-Expected-Priority 这种字段
- ❌ "queued / in_progress / delivered" 这种 status 名称（除非他主动看 TaskBar）
- ❌ "你刚创建了一个新 task" 这种系统提示

用户**会看到**：
- ✓ chat 主轴的自然对话流
- ✓ canvas 上长出来的 reasoning DAG（agent 在工作）
- ✓ TaskBar 上的 slim status indicator（"⟳ in_progress · 24s"）+ 干预按钮
- ✓ deliverable 节点出现时的"完整结果在画布上 →"指引

## 5. 用户的干预能力

L.E.E.K 设计上**鼓励用户干预**——这是 manager 模式的体现。

### 5.1 实时干预（in_progress 期间）

干预方式有两种 surface：

**TaskBar 上的 control 按钮**（chat 主轴顶端）：

```
┌─────────────────────────────────────────────────────────────┐
│ ⟳ in_progress · 评估 NVDA 加仓 · turn 3/5 · 24s            │
│ Agent 当前: corpus.search "margin of safety"               │
│ [追加约束] [中断]                                            │
└─────────────────────────────────────────────────────────────┘
```

**Chat 输入框直接说**：用户在 thread 里输入"也考虑 BABA"——agent 智能识别为约束。

每种干预都是一个 control 命令（HTTP POST 或 WebSocket frame），由 agent loop 在下一个 yield point 接收。

### 5.2 重指派 / fork（P1 简化版，P2 强化）

P1：用户对 deliverable 不满意时，在 chat 框说"基于这个再换个 angle 看看"——agent 自动 fork 一个新 task（带原任务 context）继续工作。

P2 / multi-agent 升级后：用户可以"把这个任务交给一个新组合的 agent 团队"——届时再设计。

### 5.3 中断的语义

- 中断 ≠ 删除：任务进 `cancelled` 状态，所有已产出的 reasoning / panel / artifact 都保留
- 中断后用户可以"恢复"——从 cancelled 状态 → in_progress（continue from where left off）
- 中断的数据完整性：agent 在每个 yield point 持久化 scratchpad / 已 spawn subagent 状态，便于恢复

## 6. Mandate = Team Charter

之前的设计里 "mandate" 是用户的投资准则。在 manager 模式下，它升级为 **Team Charter**——团队工作章程。

### 6.1 Team Charter 的内容

```yaml
# 用户的投资风格
style:
  - long-term-fundamental    # 长期基本面
  - margin-of-safety         # 重视安全边际
  - quality-over-cycles      # 偏好质量公司

# 硬约束（agent 不能逾越）
hard_limits:
  max_position_pct: 10        # 单标位置不超过 10%
  max_drawdown_tolerance: 25  # 最大回撤容忍 25%
  forbidden_instruments:      # 禁止涉及的工具
    - options
    - leveraged_etfs
    - crypto

# 软偏好（agent 可以建议但要 flag）
soft_preferences:
  preferred_sectors:
    - tech
    - consumer-staples
  avoid_sectors:
    - tobacco
  avoid_geos:
    - russia
    - belarus

# 工作风格（影响 agent 输出形态）
work_style:
  decision_verbosity: detailed    # 简洁 / 标准 / 详细
  cite_corpus_always: true        # 决策必须引用 corpus
  challenge_my_bias: true         # agent 要主动质疑用户的偏见
  morning_brief_time: "08:00"     # （未来）晨报时间
```

### 6.2 mandate_violations 是 deliverable 的一等公民

每个 deliverable（特别是 decision draft）必须经过 mandate check，violations 显式列出：

```
Mandate Check:
  ✓ size 1.5% < 10% 上限
  ⚠️  集中度警告：科技股已 65%，若 confirm 此次加仓将到 67%
  ✓ instrument: 普通股 (allowed)
  ⚠️  challenge_my_bias: 你最近 6 个月所有加仓都在科技股，是否考虑分散？
```

violations 不阻断 deliverable 提交——但用户必须**显式看到**才能 confirm。这与 ChatGPT 模式的"AI 给一个完美建议"形成对比——L.E.E.K 是**有立场的团队**，会和你 challenge。

## 7. Reactive / Proactive / Browse 三模式与 Task 的关系

之前 concept.md 里定义了三种使用模式。在 task framing 下重新整理：

### 7.1 Reactive（用户下达任务）

- 用户在 chat 主轴输入自然语言 → main agent 隐式提取 task → 在 canvas 上工作 → 输出 deliverable 节点
- 这是最主要的工作流（80%+ 使用场景）
- **前端不暴露 task 概念**——没有 task creator form / task board，agent 自动从 user message 推断 goal / constraints / expected_deliverable（详见 §4 输入形态）

### 7.2 Proactive（系统创建任务给用户）

- Cron 触发：mandate 配置的"每周复盘"、某 decision 到 review 期
- 系统创建 task（status=queued），打开 leek 时用户看到 "你有 N 个待处理任务" banner
- 用户可以：立即处理 / 延期 / 取消
- **agent 不会越过用户自动开始**——除非任务被 mandate 标记为 "auto-execute"（P1 不做）

### 7.3 Browse（无目的探索）

- 进 Corpus Brain 全景 / 浏览 Watchlist / 翻历史 sessions
- **不创建任何任务**——纯浏览
- 浏览过程中如果产生兴趣，可以一键 "为这个起任务"

三模式的边界很清晰，task 是 reactive + proactive 的产物，browse 不产生 task。

## 8. Chat thread 与 Task 的关系

任务总属于某个 session（chat 主轴线程）。每个 session 可以有多个 task。

```
Session "周一晨会"
├─ Task #1: 评估 NVDA 加仓                    [delivered → confirmed]
├─ Task #2: 看一下 BABA 最近公告              [delivered]
└─ Task #3: 复盘上周 META 决策                [in_progress]

Session "深度调研：AI 半导体"
└─ Task #4: 对比 NVDA / AMD / TSM 估值        [in_progress]
```

- **Session 是 chat thread 的容器**——是用户的工作空间
- **Task 是 session 内的工作单元**——一个 session 可能涵盖多个相关任务
- 用户可以在 session 内"开新任务"或"切到现有任务"
- Session 与 task 都持久化在 vault

## 9. Deliverable 的形态

Deliverable 是 task 的产物。它**不是 chat 文字消息**——是 canvas DAG 上的 typed 节点（含完整 form），用户可 confirm/reject。

### 9.1 P1 支持的 deliverable 类型

| 类型 | 内容 | DAG 节点 typed |
|--|--|--|
| `decision_draft` | 决策草稿（ticker / 方向 / 仓位 / 止损 / 期限 / 复盘 schedule / rationale / corpus refs / mandate check） | decision_draft 节点 |
| `research_brief` | 调研简报（要点列表 / 数据图表 / 来源 / corpus refs） | research_brief 节点（内嵌多个子节点） |
| `review` | 复盘（评分 / 教训 / corpus 候选） | review_draft 节点 |
| `comparison` | 多标的对比（表格 / 雷达图） | comparison 节点 |
| `morning_brief` | 晨报式摘要（今日要点 / portfolio 关注 / news / 待处理任务） | research_brief 节点 |
| `free_form` | 自由格式（agent 自己决定） | final_reply 节点 + 一组 observation 节点 |

### 9.2 Deliverable 的状态

- `draft` （agent 写到一半）
- `ready` （agent 完成，等 user review）
- `confirmed` （user 接受）
- `rejected` （user 拒绝；可选写理由）

### 9.3 Deliverable 与 vault 的关系

- 所有 deliverable 都进 `vault.deliverables` 表（永久保留）
- 某些类型 trigger 副作用：
  - `decision_draft` confirmed → 同步进 `vault.decisions`
  - `review` confirmed → 同步进 `vault.reviews`
- rejected 的 deliverable 也保留——历史教训 / 复盘材料

## 10. 与 ChatGPT 模式的关键差异（总结）

> 这一节给 UX 设计师特别强调，避免把 L.E.E.K 设计成另一个 chatbot。

| 差异点 | ChatGPT | L.E.E.K |
|--|--|--|
| 主输入是什么 | chat 文本框 | **chat 文本框**（同形态但语义不同——见下） |
| 中间过程在哪 | chat 流里的 message bubble | **canvas Reasoning DAG**（chat 主轴只显示最终回复） |
| Agent 输出长什么样 | 文字回复 | **DAG 上的 typed deliverable 节点** |
| 用户怎么"完成"一次交互 | 看完文字就结束 | **review → confirm/reject** 是必经动作 |
| Agent 能不能 challenge 用户 | 偶尔（受 RLHF 训练影响） | **必须**（charter.challenge_my_bias） |
| 中途能不能干预 | 只能停止生成 | **追加约束 / 中断 / chat thread 内追问** |
| 历史能不能影响后续 | 限于 chat history token | **vault 中所有历史决策 / 复盘 / corpus 引用都是持久 context** |
| 用户的"准则"在哪 | system prompt（用户看不见） | **Team Charter（可视化编辑、显式应用）** |
| Agent 拒答的边界 | "我不能给具体投资建议" | **agent 必须给出立场和量化建议**（用户是经理，要为决策负责） |

L.E.E.K 设计上**不规避具体建议**——但所有建议都在 charter 框架下，且**最终决策权属于用户**（confirm 这个动作）。

## 11. 给 UX 设计师的核心要点

1. **主输入是 chat 文本框**——但中间过程绝不塞进 chat 流。canvas DAG 才是 agent 工作过程的家
2. **Canvas = 流动的 Reasoning DAG**——节点类型化（typed），位置由布局算法决定，用户不能拖动 / 钉住 / 关闭
3. **Task 概念在前端隐式**——没有 task creator form / task board，agent 自动从 user message 提取
4. **TaskBar 是 chat 主轴顶部 slim 状态条**——只显示当前 task status + 干预按钮，不让 task 概念变重
5. **进度可视化是核心 craft**：canvas DAG 流式展开 / corpus brain 激活 / subagent 子分支——让用户**看到团队在工作**
6. **干预按钮显式存在**——TaskBar 上的 [追加约束] [中断] 是 first-class，不藏在右键菜单。manager 是要"指挥"的角色
7. **deliverable confirm/reject 是仪式性动作**——视觉上要有"我做了决策"的重感（不是随手的 like 按钮）
8. **Team Charter 的可视化编辑器**——用户表达自己的入口，做得好会让用户感到 ownership
9. **Subagent 透明度**：subagent 是 lead 的实现细节，但**用户能在 canvas DAG 里看到**（📌 容器节点），不是黑盒。透明 ≠ 用户管理

## 12. 后续依赖文档

- [`decisions/0010-single-agent-coordinator-subagent.md`](decisions/0010-single-agent-coordinator-subagent.md) —— 架构决策：单 agent + subagent map-reduce
- [`frontend/concept.md`](frontend/concept.md) —— chat × canvas DAG 完整形态
- [`frontend/panels.md`](frontend/panels.md) —— canvas DAG 的 typed 节点 + 独立元素清单
- `p1-spec/data-schema.md` —— vault 中 tasks / deliverables / subagent_runs 的 schema（task 仍存在于后端，只是前端隐式）
- `p1-spec/agent-loop.md` —— harness 如何执行 task / 如何调度 subagent
- `p1-spec/api.md` —— task lifecycle 的事件协议
