# L.E.E.K Design System

> Category: A-share research workbench · agent-driven investment workflow
> Direction: **Modern Minimal** (Linear / Vercel / Notion 系) · dark-mode native
> Base brand: forked from `nexu-io/open-design/design-systems/linear-app`,
> adapted for Chinese A-share research conventions.

## 1. Visual Theme & Atmosphere

L.E.E.K 是 A 股投研 agent 工作台 — 用户用它做严肃投资研究,看的是 facts 不是
opinion。视觉气质 = **engineering-clean + 信息密度恰当 + 中文阅读舒适 + 深夜
研究友好**。

不要 Bloomberg Terminal 那种焦虑感、不要同花顺那种花花绿绿的廉价感、不要
Notion 那种过于"温柔"的不严肃感。借 Linear 的精确感:dark canvas + 几乎不
可见的 ultra-thin border + 克制的 accent + Inter Variable 工程字体 + 数字
等宽对齐(`tabular-nums`)。

**核心特征**:
- **Dark-mode native**:`#08090a` 深底色,信息密度由 luminance gradation 表
  达,不用强对比色
- **红涨绿跌**(中国 A 股惯例):覆盖 Linear 系统中"success 绿/danger 红"
  的语义,在金融数据展示场景中 **`--up`=红 / `--down`=绿** 是法定
- **多字体栈中文友好**:Inter Variable(英 / 数字)+ "PingFang SC" /
  "Hiragino Sans GB" / "Microsoft YaHei"(中文)+ "Berkeley Mono" / SF Mono
  (代码 / 数字 / ticker code)
- **tabular-nums**:所有金融数字(价 / 涨跌 / PE / PB / 涨跌幅 / 成交量)启用
  `font-variant-numeric: tabular-nums`,让表内数字垂直对齐
- **Indigo accent** 用作"agent 在干啥"的语义色(loading / processing /
  highlighting),**不用作涨跌**
- **Status 绿** (`--ok`) **保留** 用于"工具完成 / 已写入 corpus"等非金融状态
- **Card-as-content**:canvas 是 agent 思路展示区,每张工具卡片用合适宽度
  (240-600px),不要一律占满 canvas 宽度

## 2. Color Palette & Roles

### Background Surfaces(3-level dark)
- **`--bg`** `#08090a` — workbench 主底色(canvas / 大区底色)
- **`--surface`** `#191a1b` — 卡片 / dropdown / chat bubble 抬升表面
- **`--surface-2`** `#28282c` — hover / 选中 / 嵌套卡片二级表面
- **`--panel`** `#0f1011` — Rail / sidebar 面板背景(比 bg 略亮)

### Text(4-level luminance)
- **`--fg`** `#f7f8f8` — primary text(标题 / 主要内容)
- **`--fg-2`** `#d0d6e0` — secondary(body / 卡片描述)
- **`--muted`** `#8a8f98` — tertiary(placeholder / metadata / 数据来源 cite)
- **`--meta`** `#62666d` — quaternary(timestamp / 禁用态 / 微标签)

### Border(semi-transparent white)
- **`--border`** `rgba(255, 255, 255, 0.08)` — 卡片标准 border
- **`--border-soft`** `rgba(255, 255, 255, 0.05)` — 表格行分隔 / inner divide
- **`--border-strong`** `rgba(255, 255, 255, 0.12)` — focus / active 状态

### Brand & Accent
- **`--accent`** `#5e6ad2` — leek 品牌色(主 CTA / agent loading 脉冲 /
  brand 标记)
- **`--accent-on`** `#ffffff` — accent 上的 text
- **`--accent-hover`** `#828fff` — interactive hover
- **`--accent-active`** `#4752c4` — pressed state
- **`--accent-tint`** `rgba(94, 106, 210, 0.12)` — subtle 背景 tint
  (selected row / active tab indicator)

### Financial Data Colors(中国 A 股 红涨绿跌)
- **`--up`** `#ef4444` — 涨(price up / positive change / market open red)
- **`--up-tint`** `rgba(239, 68, 68, 0.14)` — 涨幅 chip / cell tint
- **`--down`** `#10b981` — 跌(price down / negative change)
- **`--down-tint`** `rgba(16, 185, 129, 0.14)` — 跌幅 chip / cell tint
- **`--flat`** `var(--muted)` — 持平 / unchanged

> ⚠ **覆盖 Linear schema 的语义**:Linear 系统里 `--success` 是 `#27a644`
> 绿,用于"已完成 / 在进行"。leek 保留 `--ok` 同含义(见下),但 `--up` /
> `--down` 是 **A 股金融语境专用**,跟 status 信号严格分开,不要混用。

### Status Signals(非金融)
- **`--ok`** `#10b981` — 工具调用成功 / 写入 corpus / 操作完成
  (NOTE: 颜色跟 `--down` 同,但语义分开 — `--ok` 永远不出现在涨跌行情
  cell 里,`--down` 永远不出现在工具状态 chip 里)
- **`--warn`** `#eab308` — 数据缺失 / 半僵尸接口 / cost cap 临近
- **`--danger`** `#dc2626` — 工具失败 / fatal error / abort
- **`--info`** `var(--accent)` — agent 思考中 / queueing / retrying

## 3. Typography

### Font Stacks
- **`--font-display`** = Inter Variable / "PingFang SC" / "Hiragino Sans GB" / system-ui
- **`--font-body`** = Inter Variable / "PingFang SC" / "Hiragino Sans GB" / "Microsoft YaHei" / -apple-system / system-ui
- **`--font-mono`** = "Berkeley Mono" / ui-monospace / "SF Mono" / Menlo / Monaco / Consolas

### Type Scale(px)
- `--text-xs` 12 — micro / 时间戳 / chip
- `--text-sm` 13 — caption / metadata
- `--text-base` 14 — body 默认(中文 14px 比 16px 紧凑,workbench 信息密度更高)
- `--text-md` 15 — body 大(强调段)
- `--text-lg` 18 — 卡片标题 / 章节小标题
- `--text-xl` 22 — 章节大标题 / dialog 标题
- `--text-2xl` 28 — page 大标题
- `--text-3xl` 36 — display(罕用)

### Line height & Tracking
- `--leading-body` 1.55 — body 中英文阅读
- `--leading-compact` 1.4 — 卡片 / dense data
- `--leading-tight` 1.15 — 标题
- `--tracking-display` -0.018em — 标题字距

### OpenType
- 所有 Inter 文本启用 `"cv01" "ss03"` 几何变体(Linear 同款)
- 金融数字 `font-variant-numeric: tabular-nums` — **关键**,所有报价 / 涨跌
  / 财务表 / K 线 OHLC 数字都必须等宽

### Weight 规则
- 400 body / 510 UI(Linear 招牌 medium-tight) / 590 emphasis / 650 display
- 不用 700+ bold,信息密度由 size / tracking / luminance 拉开

## 4. Spacing & Radius

### Space scale(px)
4 / 8 / 12 / 16 / 20 / 24 / 32 / 48 / 64

### Radius
- `--radius-sm` 4 — chip / pill
- `--radius-md` 6 — button / input
- `--radius-lg` 8 — card / dialog
- `--radius-xl` 12 — large card / modal
- `--radius-pill` 9999

### Layout grid
- 主体三列(布局):Chat box / Canvas / Sidebar
- Chat 列宽:380px(min 320,max 480),不可调
- Canvas 列宽:flex 1(吃剩余宽度)
- Sidebar 列宽:280px(corpus map 256px square + plan 列表自然高度)
- Rail 宽:48px(纯 icon)
- 全局 max-width:无(workbench 吃满 viewport)

## 5. Components(关键 spec)

### Canvas Card 系统
- **变宽度**:240 / 320 / 360 / 420 / 480 / 540 / 600 / fluid
- **CardShell** 提供:左侧 icon + title + status pill + 右上 action(copy /
  expand / raw-toggle)
- **不要** 一律全宽
- **嵌套 subagent_card**:缩进 12px + 左侧 1px accent-tinted 竖线(`box-shadow:
  inset 2px 0 0 var(--accent-tint)`)
- **failed tool card**:`--border` 改 `var(--danger) at 30% alpha`,header
  icon 用 `--danger`

### Chat bubble
- User bubble:右对齐 / `--accent` 底 / `--accent-on` 字 / 圆角 8px,左下角 4px(指向 self)
- Assistant bubble:左对齐 / `--surface` 底 / `--fg` 字 / 圆角 8px,右下角 4px
- 实时打字效果:caret 用 `--accent`,blink 200ms

### Status pill(agent 当前阶段)
- 圆角 9999px / 字 12px / padding 4 12px
- 颜色:`--info`(thinking / queued / retrying)/ `--ok`(done)/ `--warn`
  (cap 临近)/ `--danger`(fatal)
- 不同阶段加 icon:思考 dot pulse / 调工具 spinner / 写答 caret blink

### Rail (左 vertical navbar)
- 48px 宽 / `--panel` 背景 / 顶部 brand icon 24px + 1 列 icon button 36px / 底部 settings 36px
- 当前 active icon 用 `--accent` color + 左侧 2px `--accent` indicator bar
- 仅 icon,无文本

### Session list drawer
- Trigger:Chat 区顶部 "≡ history" 按钮
- 弹出方式:从左滑入,宽 320px,backdrop `rgba(0,0,0,0.4)`
- 内容:session list 时间分组(today / 本周 / 本月 / earlier),搜索框 top,
  右键菜单 rename / delete
- 关闭:点 backdrop / Esc

### Settings page
- Trigger:Rail 左下角齿轮
- 占满 main 区(覆盖 chat / canvas / sidebar 三列)
- 不弹 modal,作为 page 显示
- 退出:左上 "← back" 按钮 / Esc 回 chat 页

### Corpus map widget(右 sidebar 顶部)
- 256×256 px 正方形(类 RTS 游戏 minimap)
- 力导向 graph mini view,节点缩为 dot,本 turn 激活节点脉冲
- 点小图全屏弹出大图(class lk-corpus-fullscreen)

### Plan widget(右 sidebar 底部)
- 无 plan 时整个区域不显示
- 有 plan:列表项 with status dot(in_progress 用 `--accent` 脉冲 / completed
  用 `--ok` / pending 用 `--muted`)+ 短文本

## 6. Motion

- `--motion-fast` 150ms cubic-bezier(0.4, 0, 0.2, 1) — hover / press 反馈
- `--motion-base` 250ms — drawer / tab 切换
- `--motion-slow` 400ms — page transition
- `--motion-pulse` 1500ms — agent 思考 dot pulse(2 step 0 ↔ 70% opacity)
- **不要** spring overshoot / bounce — workbench 严肃产品,反 ai-slop 那种
  弹跳浮夸的 CSS animation

## 7. Anti-patterns(明确禁止)

- ❌ Bloomberg Terminal 般密集字号 + 高对比黄绿色
- ❌ 同花顺 / 雪球 那种粉红 / 浅紫 / 浅蓝渐变背景
- ❌ Notion 灰色感的"温和到无聊"
- ❌ pure black `#000` 或 pure white `#fff` — 视觉硬伤
- ❌ 卡片 box-shadow 重灰 / 浮夸 elevation
- ❌ 弹簧动画 / bounce / overshoot
- ❌ neumorphism / glassmorphism / heavy blur
- ❌ 涨用绿、跌用红(违反 A 股惯例)
- ❌ 在工具状态 chip 里用 `--up` / `--down`(语义混淆)
- ❌ 数字不等宽对齐(表内 17.92 跟 1.5 不对齐 = 设计 fail)
- ❌ 用 placeholder Lorem ipsum 假数据 — 任何 mock 用真实股票/真公司名

## 8. Source contracts

- **Token names**:此文件 §2-§4 是 leek 的 source of truth
- **Base brand**:nexu-io/open-design `design-systems/linear-app`(Apache-2.0)
- **Design direction**:`open-design/packages/contracts/src/prompts/directions.ts`
  → `modern-minimal`
- **Universal craft contracts(适用)**:
  - `open-design/craft/color.md` — never pure black/white
  - `open-design/craft/typography.md` — type scale 节奏
  - `open-design/craft/animation-discipline.md` — 反弹簧 / 反过度
  - `open-design/craft/anti-ai-slop.md` — 反 cliche AI 视觉
  - `open-design/craft/state-coverage.md` — empty / loading / error 全覆盖

## 9. 给 implementers 的最后一句

每个新组件先问 4 件事:
1. 它要展示什么样**类型**的信息?(facts 数据 / agent 状态 / user 操作)
2. 中英文混排会不会断行 / 错位?
3. 数字是否需要 `tabular-nums`?
4. 涨跌信号用 `--up` / `--down`,不要碰 `--success` / `--danger`

每个新颜色先查 §2 是否已有 token;新增颜色必须先在此 DESIGN.md §2 命名 +
新 token,而不是直接写 hex。
