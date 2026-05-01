# L.E.E.K Frontend Concept

> chat-canvas 形态、产品愿景、用户旅程、视觉与交互原则。本文档是给 UX 设计师的入口文档——读完它应该能 cold-start 出 wireframe 和视觉稿。

## 1. 产品定位

**L.E.E.K (老韭菜) 是一个投研操作系统**，给希望停止做"市场韭菜"的散户用。它把一份策划过的投资智慧（corpus）变成可执行的研究、决策与复盘。

它对用户来说**不像一个 chat 工具**——更像一个**会和你一起思考的工作台**：

- 你提一个问题（"NVDA 现在值得加仓吗？"）
- 它一边查行情 / 翻新闻 / 翻你的笔记 / 调你的持仓，一边把过程可视化展示给你看
- 它最终给你一个有引用、有量化、有止损的决策草稿，等你 confirm
- 它记得你是谁、记得你做过的所有决定、能在你下次回来时帮你复盘

它**不是**：
- 不是聊天机器人（chat 只是入口形式）
- 不是 dashboard（数据是为思考服务的，不是反过来）
- 不是模拟交易系统（它输出决策，不下单）
- 不是一个写好的"投资 Buddy AI"——它是**你自己的投资 OS**，由你自己策划的智慧驱动

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
┌──────────────────────────────────────────────────────────────────────┐
│  L.E.E.K                                        [Settings] [User]    │
├────────────────────┬─────────────────────────────────────────────────┤
│                    │                                                 │
│   Chat 主轴         │            Canvas（动态 panels）                │
│   （时间轴）        │                                                 │
│                    │   ┌─────────────────┐   ┌───────────────────┐   │
│   用户：NVDA 还能   │   │ Corpus Brain    │   │ Quote: NVDA       │   │
│   加仓吗？          │   │ (持续可见 / 可  │   │ $480.20 ↑ 2.1%   │   │
│                    │   │  最小化)        │   │ ━━━━━━━ chart    │   │
│   Agent：让我看看   │   │                 │   │                   │   │
│   你当前的持仓...   │   │  神经元激活中    │   │                   │   │
│   ┌───────────┐    │   │  ●──●──●        │   │                   │   │
│   │ ToolCall  │    │   └─────────────────┘   └───────────────────┘   │
│   │ portfolio │    │                                                 │
│   └───────────┘    │   ┌─────────────────────────────────────────┐  │
│                    │   │ Reasoning DAG（流式展开）               │  │
│   Agent：你已经持   │   │                                         │  │
│   有 50 股 NVDA... │   │   问题 → 持仓查询 → 行情 → 估值        │  │
│                    │   │             ↘ corpus: margin-of-safety  │  │
│   [继续输入...]    │   │                  ↓                       │  │
│                    │   │              决策草稿                    │  │
│                    │   └─────────────────────────────────────────┘  │
│                    │                                                 │
│                    │   ┌─────────────────────────────────────────┐  │
│                    │   │ DecisionDraft: 加仓 NVDA  [Confirm] ... │  │
│                    │   └─────────────────────────────────────────┘  │
└────────────────────┴─────────────────────────────────────────────────┘
```

### 关键设计原则

1. **Chat 是入口，canvas 是工作面**——用户输入仍然在 chat 主轴，但**思考过程发生在 canvas**
2. **Panels 是 agent 的输出形式**——每个 panel 是一个 typed artifact（warp 风格），不是简单的"内嵌图片"
3. **Panels 长出来 / 长得见**——agent 工作时 panel 实时刷新（thinking → tool calling → result），用户看着它"长出来"
4. **Corpus Brain 是持续可见的 ambient 视图**——不是"打开 → 关闭"的临时面板，更像 chat 主轴一样常驻（可最小化但不会自动消失）
5. **历史可重放**——session 持久化，明天回来打开同一 session，不仅看到对话历史，还能看到当时的 panel 状态、当时的激活路径
6. **没有"agent 在打字"的 placeholder**——thinking 直接以 ReasoningDAG / 流式 ThoughtStream 的形式展现，不是"..."等待动画

## 4. 用户旅程

### 4.1 典型 reactive session（用户主动发问）

```
Step 1  用户打开 leek (localhost:8964)
        - 看到首页：欢迎区 + 持续 ambient 的 Corpus Brain（背景旋转、未激活）
        - 看到最近 sessions 列表 + watchlists 摘要
        - 看到当前 portfolio 摘要

Step 2  用户输入："NVDA 现在值得加仓吗？我已经有 50 股"
        - chat 主轴出现用户消息
        - canvas 立即出现一个新的 ReasoningDAG panel（起点节点：用户问题）
        - Corpus Brain 开始有节点轻微脉冲（agent 检索"加仓"、"集中度"、"科技股"等概念）

Step 3  agent 进入工作流（实时事件流）
        - ToolCall panel：portfolio 查询（看你已有持仓）
        - ToolCall panel：行情查询（NVDA 当前价、近期走势）
        - Quote panel 长出来：NVDA $480 + K 线
        - Corpus Brain 更多节点激活：margin-of-safety、kelly-criterion、概念图谱遍历可视化
        - ReasoningDAG 边继续连接：portfolio 节点 → corpus 节点 → 估值节点 → ...

Step 4  agent 给出决策草稿
        - DecisionDraft panel 出现：
          - ticker: NVDA
          - direction: long (加仓)
          - 建议加仓量：10-20 股（基于你的总仓位 + Kelly 计算）
          - stop_loss: $440
          - horizon: 90-180 天
          - rationale: 引用 3 篇 corpus 文章
          - corpus_refs（点击可展开 → 同时高亮 Corpus Brain 对应节点）
        - chat 主轴：agent 用一段话总结这个决策

Step 5  用户看决策草稿
        - 可以编辑：调整加仓量、止损、期限
        - 可以追问："如果财报暴雷会怎样？" → 触发新一轮分析
        - 可以 Confirm → decision 写进 vault，status = confirmed
        - 可以 Discard

Step 6  用户在真实账户下单后回到 leek
        - 主动更新 portfolio：NVDA 50 → 65 股
        - leek 在 portfolio panel 更新，下次 agent 看到的就是新持仓
```

### 4.2 Proactive 浏览模式（用户没问问题，只想看看）

```
Step 1  用户打开 leek，没有立即输入

Step 2  Corpus Brain 缓慢自转 / 微脉冲（暗示"在等"，可被点击进入"查看模式"）

Step 3  用户点 Corpus Brain → 进入大脑全景视图
        - 可以拖动 / 缩放 / 搜索
        - 可以点击节点 → 弹出 corpus 概念页内容
        - 可以看到节点的"近期激活历史"（"过去 7 天这个节点被你的 N 次分析激活过"）

Step 4  用户点击某个有意思的概念（如 "owners-earnings"）
        - 弹出 Article panel 显示 corpus 内容
        - 旁边推荐："基于这个概念，你想看哪些当前持仓的解读？"
        - 用户点 → agent 按这个概念分析当前 portfolio
```

### 4.3 复盘场景

```
Step 1  cron 触发：3 个月前的 NVDA 决策到 review 期了
        - 浏览器开 leek 时看到顶部 banner：1 个待复盘
        - 点击 → 跳转到该 decision 的 session

Step 2  agent 自动准备复盘 context
        - 拉当时的 decision 内容
        - 拉决策时点的 portfolio
        - 拉 ticker 这 3 个月的行情走势
        - 拉决策引用过的 corpus 节点（Corpus Brain 高亮回放路径）
        - 触发 ReasoningDAG："当时的判断是什么 / 现实如何 / 差距在哪 / 哪些 corpus 概念可以更新"

Step 3  agent 生成 review draft
        - 评分（1-5 自评 + agent 评）
        - lessons learned
        - 是否需要写进 corpus inbox 的候选（P1 不实现自动写，但可以在 review panel 标记）

Step 4  用户编辑 review → 写入 vault
```

## 5. 视觉与交互原则

### 5.1 视觉

- **暗色主题为默认**（金融工作场景，长时间盯屏，护眼）；亮色主题作为可选
- **数据 first**：数字 / 图表 / 节点是主角；装饰性视觉元素（背景、icon 风格、阴影）极简
- **彩色保留给"信号"**：上涨绿 / 下跌红 / 激活金黄 / 警告橙 / 错误红——其他 UI 都是灰阶
- **字体**：等宽（金融数字用 SF Mono / JetBrains Mono / IBM Plex Mono）+ 正文 sans-serif
- **空间**：呼吸感优先于"信息密度最大化"——chat-canvas 不是 Bloomberg dashboard
- **动效**：所有动效服务于"理解发生了什么"——节点出现 / 边连接 / 数据更新都有微动效，但不超过 300ms
- **视觉层次**：Corpus Brain + ReasoningDAG 是核心舞台，其他 panel 是"工具台上的工具"

### 5.2 交互

- **键盘优先**：ctrl+k 打开 command palette、`/` 聚焦输入框、`@` 触发 mention（@ticker / @decision / @corpus-concept）
- **panel 操作**：所有 panel 可拖动 / 缩放 / 最小化 / 钉住 / 关闭
- **panel 布局**：默认网格自动 layout，用户可自由摆放（save layout per session）
- **mention chip**：在 chat 输入框 `@NVDA` 触发 ticker chip，`@corpus:margin-of-safety` 触发 corpus chip——这些 chip 会被 agent 作为强 context
- **拖拽**：从 Watchlist 拖一个 ticker 到 chat 输入框 = 自动插入 mention chip
- **激活联动**：点击 ReasoningDAG 节点 → CorpusBrain 同步高亮关联节点；反之亦然

### 5.3 实时性的视觉表达

- 流式接收 LLM 输出 → chat 主轴文字逐字 / 逐 token 出现（不是 "..." 然后整段砰一下）
- ToolCall panel：从 "calling..." → "running..." → "result ready" 三态可见
- ReasoningDAG：节点逐个出现，边逐个连接，**当前活跃节点持续脉冲**直到下一个节点接力
- 高频 quote 数字：textContent 直接更新 + 最近一次变化方向用极淡的 flash（绿/红 100ms 渐隐）
- Corpus Brain 激活：节点脉冲峰值 ~150ms，扩散 ripple 持续 ~500ms

## 6. Reactive vs Proactive 共存

L.E.E.K **不假设**用户"知道自己想问什么"。两种工作模式都是一等公民：

### Reactive（用户主动发问）
- 经典 chat 流
- 用户输入 → agent 思考 → 输出
- 大部分 user journey 走这条

### Proactive（agent / 系统提示）
- Cron 触发的 review reminder
- Corpus Brain 的"近期激活热点"展示（让用户被有趣的概念吸引）
- Watchlist 的"今日动静"摘要（如果用户开启）
- **不做**：daily briefing 主动推送（推迟，handoff §5 延伸 #7）

### Browse 模式（无目的探索）
- 用户进 Corpus Brain 全景视图，自己浏览
- 点节点 → 看 corpus 内容
- 沉浸感设计：不强迫用户立即"问问题"

## 7. 用户身份与 mandate

每个用户启动 leek 时，gateway 接受 user_id（默认 OS 用户名）。所有 vault 数据按 user_id 隔离。

用户的**投资准则（mandate）**是 leek 的硬约束 prompt：
- 默认 mandate：长期权益 / 不碰复杂衍生品 / 单标位置上限 10% / 最大回撤容忍 -25%
- 用户可在 settings 里编辑（前端给一个可视化编辑器）
- agent 输出 decision 时**必须**经过 mandate 检查（"这个决策的 size 超过单标上限了"→ agent 自动调整或提示用户）

mandate 的可视化是产品的另一个温柔的 detail——让用户感到"这个 AI 是为我服务的，不是泛泛的投资建议"。

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

1. **Corpus Brain 是产品 signature moment**——花最多 craft 在这里。它不是"另一个 sidebar"，是产品灵魂。
2. **Reasoning DAG 是它的孪生**——两者必须能联动 / 视觉风格一致 / 交互对称
3. **流式动效不超过 300ms**——再长就阻塞用户感知
4. **panel 类型是开放的，不是固定的**——用户应该感到 "agent 决定召唤什么 panel"，而不是"我在 7 个固定 tab 里点选"
5. **数据 first，装饰极简**——不要 dashboard 风格的繁复 widget；金融人对花哨视觉敏感（觉得"不专业"）
6. **暗色为默认**——金融工作场景常态
7. **键盘优先**——目标用户是有耐心 / 注重效率的散户，键盘文化欢迎
8. **不像聊天工具**——chat 主轴是入口，但视觉重心在 canvas，让用户感觉"在工作"而非"在聊天"

## 11. 后续依赖文档

- `frontend/panels.md` —— 完整 panel 清单 + 每类 panel 的详细设计
- `frontend/tech-stack.md` —— 技术栈（SolidJS / Tailwind / Kobalte / Canvas 库选型）
- `p1-spec/api.md` —— 前端要消费的事件协议
- `p1-spec/agent-loop.md` —— agent 的 phase 模型决定 panel 的状态机
