# L.E.E.K — 项目功能简报 · 给 UI/UX 设计师

> 2026-05-28。此文档面向 **Claude Design**(或任何 UI/UX 设计师),让其在不读源码的前提下能完整理解 leek 是什么、用户是谁、有哪些功能面、当前 UX 痛点,然后据此设计完整的视觉系统 + 信息架构 + 关键交互流。**这不是工程文档,是设计输入。**
>
> 设计师产出形式:Figma / 可交互原型 / 截图 mockup / 设计 spec(取决你方便)。我们这边收到后会按你定的 spec 重新实施前端(SolidJS,可全部重写,无遗产负担)。

---

## 1. 一句话定位

**LEEK 是单用户 local app 形态的 A 股投研 agent 工作台。** 用户在本机跑 leek,通过自己的 codex token call LLM,leek 用 9 个 facts-only 工具去 tushare / 东方财富等数据源取数,在用户自己积累的 corpus(笔记 / wiki)视角下做分析,产出可用于投资决策的研究输出(行情速判 / 深度复盘 / 横向对比 / 大盘观察)。

**不是**:
- 不是 SaaS(没有云后端、没有登录、没有团队功能)
- 不是 chatbot 玩具(用户用它实打实做研究,产出会作交易依据)
- 不是 Bloomberg / wind(不是付费金融终端的 clone,信息密度比终端低,但深度比同花顺研报高)
- 不是社区(没有"大 V 推荐 / 评论 / 跟单"功能)

---

## 2. 用户画像

**典型用户**:
- 中国 A 股个人投资者(散户 / 中小资金长线)
- 有自己的投资体系、积累过笔记、读研报有判断力
- 不只看 K 线,做基本面 + 估值 + 行业格局综合判断
- 痛点:做 1 次深度复盘要在 5+ 工具之间反复切(同花顺看行情、东方财富看 F10、雪球看观点、巨潮看公告、wind 看预期),手抄数据,自己脑补结论,半天一只股
- 期望:把研究问题给 leek,它自己取数 + 调度 corpus + 给中立 facts,**用户自己下投资判断**

**使用场景**:
1. 盘后:深度复盘某只股(15-30 分钟一只)
2. 盘中:快速看下大盘 / 某只股 / 行业资金面(< 2 分钟)
3. 跟踪:看自选股最近事件 / 卖方分歧 / 政策影响
4. 横向:几只股横向对比

**不是用户**:
- 量化交易者(不需要 LLM,需要回测平台)
- 纯短线技术派(他们要 level-2 / 涨停打板,leek 不专攻这块)
- 完全不懂投资的小白(leek 给 facts 不给 buy/sell 推荐,小白接不住)

---

## 3. 技术约束(给设计师知道边界)

- **本地 web app**:Rust gateway(axum + sqlite)+ 前端 SolidJS + TypeScript
- **完全可重写前端**:当前 UI 是为功能跑通做的,视觉无负担。你设计什么我们就重写什么
- **暗色为主**:投研人夜盘后跑得多,暗色 default 必须保留;但需要**额外提供亮色 mode**,日间用
- **中文为主**:界面文案中文,但股票代码(`600519.SH`)/ ticker / 工具名等保留英文
- **响应式**:桌面优先(1280-1920px),平板可用(800-1024px),**不要求手机适配**(本机 app)
- **SSE 实时流**:agent 写答用 server-sent events 实时 token-by-token 显示(类似 ChatGPT)
- **支持 markdown 渲染**:assistant 输出大量 markdown(表 / 列表 / 代码 / 引用 / 链接)
- **不支持 plugin marketplace**(local app 无社区)

---

## 4. 现有 4 + 1 功能面板(workbench)

### A. Chat panel(左 1/3,主交互)

**用户视角**:这是问问题 / 看答案的地方。

- 顶部:session 标题(可编辑)
- 中部:turn 流(user message + assistant reply 上下交替)
- 底部:composer(多行输入框 + 发送按钮)+ turn 状态栏

**关键交互**:
- 输入框支持 Enter 发送 / Shift+Enter 换行
- assistant 答用 markdown 渲染,实时 token-by-token 流式
- 长答可滚动,顶部按钮"回到底部"
- 每个 turn 一个 bubble(user 右 / assistant 左 现版),可显示展开/折叠
- **状态 pill**(M3.2):turn 进行时显示当前阶段(`正在启动…` / `正在调用 stock_overview` / `正在搜索网页` / `正在写回答` / `重试中 1/5` / `排队中` / `深度思考 N 秒` / `已用 N 秒`)+ 强制停止按钮
- **优雅退化卡**(M3.7):turn 撞 wall_clock / cost cap / 网络故障时,显示**部分答** + **明确说明为啥停**(`subagent 工具调用上限中止` / `codex 网络静默 180s` 等)+ "重试本回合"按钮
- "重试本回合"按钮:不丢上下文,重新发同 prompt

**状态变体设计需要的**:
- Empty(刚开新 session)
- 等待 agent 思考(0-5s)→ 短 thinking 动画
- 思考久了(5-60s)→ "agent 正在思考…"
- 深度思考(60s+,可能 1-15 分钟)→ "深度思考中(codex 内部多次搜索,无中间反馈)"
- 调工具中 → 工具名 + spinning
- 排队中(codex semaphore 限并发,排队了)→ "排队中,前面 N 个 turn"
- 重试中 → "上游 codex 临时故障,重试中 1/5,剩 X 秒"
- 写答中 → 实时打字效果
- 完成 → assistant 答 + 时间戳
- 失败优雅退化 → 卡片显示原因 + 部分答 + 重试按钮
- 失败硬故障(rare,M4.1.6 已兜底 panic)→ 错误条 + 调试链接

### B. Canvas panel(右上 2/3 上半,工具调用可视化)

**用户视角**:agent 调了什么数据工具,数据原样在这里看。

- 每个工具调用一张卡(沿时间线滚动)
- 卡片头:工具名 / 入参 / 状态(进行中 / 完成 / 失败)
- 卡片体:工具返回的 raw + distilled 数据
- 嵌套 subagent 卡可展开看内部步骤

**目前卡片粒度**(M4.1 共 9 种 + 失败卡):

1. **stock_overview**:个股 dossier — 6 节(overview / valuation / business / holders / financial / technical / corp_action),focus 决定哪节展开
   - overview:实时报价 + 简介 + 估值核心 + 行业 + top 3 概念 + 最近大事 1 行
   - valuation:PE/PB/PS/股息率历史 3 年分位 + 行业中位
   - business:主营业务收入分项(产品/行业/地区切分)+ 毛利率
   - holders:十大股东 + 十大流通 + 户数变化 + 实控人 + 机构持仓
   - financial:三大表关键科目 + ROE/毛利/资负 / 现金流质量
   - technical:K 线 60 日 + MA5/20/60 数值 + RSI 数值(**raw 数值,不下判断**)
   - corp_action:业绩预告 + 业绩快报 + 下次披露日历

2. **industry_landscape**:行业全景 — 5 focus(leaders / valuation / capital_flow / concepts / index)

3. **market_overview**:大盘 — 5 focus(snapshot / capital_flow / hot_industries / extreme_movers / north_money)

4. **recent_actions**:个股事件时间流 — 倒序,9 类 filter(公告/增减持/分红/解禁/大宗/龙虎/质押/回购/调研)

5. **market_pulse**:实时 batch 报价 + 主力资金 + 北向 + 技术原始数值(1-10 只)

6. **research_sentiment**:卖方 facts(一致预期数字 + 评级分布 + 研报标题 list + 机构调研)

7. **macro_indicators**:宏观 — 6 focus(inflation / growth / money / policy / calendar / intl_rates)

8. **read_pdf**:通用 PDF 阅读 — text 输出 + 翻页 hint(`offset=N` 续读)

9. **chart_data**:K 线 OHLC raw 数据(目前给 chart 渲染用,M4.2 是这块设计重点)

**特殊卡**:
- `subagent_card`:agent 委派给子 agent(quick-screen / deep-review / comparison / corpus-expert)时显示,可展开看 subagent 内部所有步骤(可能嵌套 2-3 层)
- `codex_duplicate_warning`:codex 内置搜索反复抓同 URL 的警告(M3.1)
- `corpus_search` / `corpus_read`:从 user 自己的笔记库搜/读卡
- `web_fetch`:leek 自己的 HTTP fetch 工具卡
- `failed_tool`:工具调用失败的红色卡(可 toggle 隐藏)
- `plan_widget`:agent 用 update_plan 工具自维护的 todo list(M1.9)— 现在在 Canvas 内,也可以挪出来独立悬浮

### C. Corpus Brain(右下,可切换 Canvas)

**用户视角**:看 leek 在用户自己积累的"投资 corpus" 上是怎么思考的。

- 视图:力导向 wiki graph
- 节点:corpus 中的 wiki 文档 / 投资原则 / 关键概念 / 公司 / 行业等(目前 296 文档 / 128 节点 / 4973 边)
- 边:跨引用
- **三层激活态**:
  - 历史(浅灰):本 session 早期 turn 用过
  - 本 turn 用过:highlight 蓝色
  - 当前正在用:脉冲动画(corpus_search / corpus_read 在 fire)
- 点节点 → 预览卡片(浮窗)+ 跳到 source 文件按钮

### D. Plan widget(右下悬浮,M1.9)

**用户视角**:agent 当前在想啥步骤。

- agent 用 update_plan 工具维护一个 todo list,每步显示进度(已完成 / 当前 / 待办)
- 显示在 canvas 右下角,可折叠

### E. Settings(头部齿轮)

**用户视角**:能改 leek 行为的旋钮。

- 各 guard 阀值:
  - idle_timeout_secs(default 180)
  - wall_clock_secs(default 1800,30 分钟硬终止)
  - max_iterations(default 50)
  - **max_tool_calls_per_turn**(default 30,M4.1.5)
  - cost_cap_usd(default 5.0)
  - doom_loop_threshold(default 3)
  - auto_compact_threshold(default 0.9)
  - builtin_url_warn / abort thresholds(M3.1)
- token:tushare_token / LEEK_WEB_SEARCH env override
- reasoning effort 下拉:`minimal` / `low` / `medium` / `high` / `xhigh`(default `medium`,M3.7 设)
- 每个字段显示"当前生效值"+ "是不是被 env var override"+ "改下次 turn 生效"

### + Sidebar(左)

**用户视角**:session list。

- 当前 session list(平铺,创建时间倒序)
- 新建 session 按钮
- 每条 session 显示标题 + 最近 turn 时间 + 状态(running / done / fatal 等)

---

## 5. 用户 session 旅程 — 4 个典型流(设计要 cover 的核心场景)

### 流程 1:盘后深度复盘一只股(15-30 分钟,用户最常用)

1. 用户新开 session,标题 "茅台 5 月深度复盘"
2. composer:`深度复盘 600519 贵州茅台:基本面 + 估值 + 主营 + 大股东 + 最近事件 + 卖方一致预期`
3. agent 内部决定委派 deep-review subagent
4. canvas:很快出现 `task → deep-review` subagent_card(灰色 placeholder + spinner)
5. canvas:subagent 内部按序 / 并发 fire 多个 stock_overview / recent_actions / research_sentiment / corpus_search 等卡片(实时浮现,**每个卡片 user 都能在过程中查看**)
6. corpus brain:相关节点(`wikis/principles/...`)持续脉冲
7. chat:bubble 显示 "正在思考 / 正在调 X 工具",中间还有 update_plan 来 narrate 进度
8. 5-15 分钟后 agent 综合所有数据写最终复盘(中文 markdown 表 + 段落 + cite 数据来源)
9. user 可:
   - 滚 canvas 查具体某节数据来源
   - 点 corpus 节点验证 leek 引的"投资原则"
   - 选 assistant 答的某段 → 复制到自己笔记
   - 让 leek 续问("再看下行业横向对比")

### 流程 2:盘中快速看下大盘(< 2 分钟)

1. 用户新 session 或在最近 session 续问
2. composer:`大盘怎么样?涨停几家?北向资金?`
3. agent 一次或几次 market_overview 不同 focus
4. chat:200-500 字市况速判 + cite 数据
5. canvas:几张 market_overview 卡片 + 指数实时报价

### 流程 3:看研报 PDF 全文(M4.1 read_pdf)

1. 用户问 `茅台最新研报怎么说?`
2. agent 调 research_sentiment 取研报列表
3. canvas:research_sentiment 卡(研报标题 + 机构 + PDF URL list)
4. user 点 PDF URL → leek 自动 `read_pdf` → canvas 出 PDF text 卡片(显示前 4000 字 + 翻页 hint)
5. chat:agent 总结研报核心观点 + cite 页码

### 流程 4:横向对比 3-5 只股(M2.7 嵌套 subagent)

1. composer:`对比贵州茅台/五粮液/泸州老窖 三家白酒龙头的基本面 + 估值 + 资金面`
2. agent 委派 comparison subagent
3. canvas:`task → comparison` depth-1 subagent_card
4. comparison 内部并发 3 个 `task → quick-screen` depth-2 subagent_cards(并排或嵌套)
5. canvas:实时看到 3 个 quick-screen 各自的 stock_overview / market_pulse 工具调用
6. 5-10 分钟后:对比表 + 短结论

---

## 6. M4 子 milestone(本次设计要 cover)

### M4.2 数据可视化(本次设计核心)

把 raw JSON 卡片升级成可视化:
- **K 线图**:OHLC + 成交量 + MA + 缩放;支持日 / 周 / 月切换;支持复权;**关键 — 这是用户最常看的图**
- **估值带**:PE/PB 历史 3-5 年分位走势 + 当前 vs 中位 / 高分位 / 低分位
- **资金流条形 / 漏斗**:超大单 / 大单 / 中单 / 小单 净流入对比
- **同行业横向 bar chart**:top 10 公司各指标对比
- **主营业务饼图 / treemap**:收入分项 + 毛利率
- **业绩趋势线**:营收 / 净利 / ROE 近 5 年
- **概念热度散点**:概念 × hot value × 当日涨幅
- **行业指数走势**:申万行业指数 60-180 日
- **持股变动**:北向 / 机构 / 户数变化时间序列
- **公告事件时间线**:垂直时间轴显示事件 + 类型 chip

设计需要:
- 风格统一(投研系,克制,避免花哨)
- 涨跌色:中国市场 **红涨绿跌**(跟欧美相反,必须遵守中国习惯)
- 数字精度:股价 2 位小数,百分比 2 位小数 + 符号,大额数字带"亿"/"万"单位
- chart 卡可全屏放大
- chart 标注数据来源(M4.1.1 facts-only 原则:cite 不能丢)
- chart 鼠标 hover 显示 tooltip

### M4.3 整体设计 polish(本次设计也要 cover)

- typography 系统(中英文 mix 友好,数字等宽对齐)
- spacing / color / shadow / radius / motion 系统
- session list 改 grouping(today / 这周 / 这月 / earlier)+ 搜索 + 重要置顶
- empty state / loading state / error state 全套设计
- 主题 toggle:暗色(default,投研夜盘)+ 亮色(日间)
- 微交互(hover / focus / transition)
- 顶部 navbar 设计(session 切换 / 设置入口 / 主题切换)
- ⚠ chat / canvas 比例 user 可调(目前固定)

### M4.4 可观测性 Dashboard(本次设计也要 cover)

新页面,user 看 leek 帮自己干了啥:
- 本周 / 本月 leek 看了几只股 + 几次 deep / quick / comparison
- 平均 wall_clock + cost_usd 分布
- stop_reason 饼图(end_turn / wall_clock_exceeded / cost_cap_exceeded / fatal_error 各几次)
- 最近 turn 列表(filter:by stop_reason / by symbol / by date)
- 最贵 prompt top 5(cost ranking)
- corpus 激活 top N(哪些 wiki 节点本周被引用最多)
- subagent 命中率(quick-screen / deep-review / comparison 各调用了几次,平均成本)

### M4.5 报告导出 + 分享(本次设计也要 cover)

- chat 内任意 assistant message → 一键导出
  - Markdown(纯文)
  - PDF(带 leek 水印 + 时间戳)
  - 微信图文格式(适合公众号粘贴)
- canvas 完整 turn → 导出 HTML report(含工具卡 + chart 图)
- 单 session 只读分享 link(M5 跨 session 才需要,M4 仅 local snapshot)

---

## 7. 当前 UX 痛点(用户视角)

1. **canvas 卡片太丑**:全是 JSON dump 灰盒,用户看不清重点
2. **没有 chart**:K 线 / 估值 / 资金流都是数字,不直观
3. **session list 平铺**:开过 50+ session 后找不到
4. **暗色看久眼累**:日间没亮色 mode
5. **状态反馈不友好**:turn 跑 5 分钟用户不知道是不是死了(M3.2 + M4.1.4 已经 emit 事件但前端没渲染)
6. **typography 没设计**:中文行高紧、数字不等宽、表格不对齐
7. **没 dashboard**:user 用了 leek 几个月不知道自己花了多少钱、看了几只股
8. **导出难**:复制粘贴出来格式丢失
9. **错误反馈生硬**:M3.7 fault 卡已有但样式简陋
10. **subagent 嵌套卡视觉混乱**:depth 2-3 时层级辨识不清

---

## 8. 设计参考(同类产品 + 设计语言)

**同行业(数据 / 信息密度)**:
- 同花顺(传统券商终端,偏老)— 行情 / F10 / 资金的视觉密度参考,但**反例:过密、灰色调、感觉廉价**
- 雪球(社区 + 行情)— 中文阅读节奏 + 卡片设计可以参考
- 东方财富 F10 网页 — F10 板块切换的信息架构可参考
- Bloomberg Terminal — 终极信息密度,**反例:不要这种焦虑感**

**SaaS 设计感(交互 / 视觉)**:
- Linear(deep work / minimal)— 暗色基调 + 微动效参考
- Notion / Cursor — 中文阅读 + markdown 渲染参考
- Vercel / Anthropic 自家文档站 — typography 参考
- Stripe Dashboard — chart 设计参考(数据可视化的克制感)

**中文阅读节奏**:
- 微信公众号 投资笔记
- 少数派 / 知乎专栏 长文阅读

---

## 9. 设计师需要决定的关键点

请在 design spec 中明确:

1. **视觉语言 mood board** — 取哪几家的元素,避哪几家的雷
2. **typography 系统** — 中英文字体配对 / 字号 scale / 行高 / 数字等宽
3. **色彩系统** — 主色 / 中性 / 涨跌(中国习惯红涨绿跌!)/ 数据可视化色板 / 暗亮两套
4. **spacing / radius / shadow** — 基础 token
5. **9 种工具卡的视觉模式** — 用模板还是各异?涨跌色怎么用?重点指标怎么突出?
6. **chart 设计语言** — K 线图风格 / 估值带 / 资金流 / 行业 bar / 业绩趋势线
7. **状态变体全套** — empty / loading / thinking / queued / retrying / writing / done / fault / panic
8. **session 列表分组 + 搜索 UX**
9. **dashboard 信息架构** — 哪些图表 / 顺序 / 滤镜
10. **导出 / 分享触点位置 + 内容样式**

---

## 10. 项目状态 + 下一步

**M4.1 已完成**:工具系统 + agent 流畅度 + 容错链全部就绪(commit `640787e`)。9 个 facts-only 工具的 raw shape 已稳定 → **设计师可信赖这些 schema 不变**。

**等设计**:M4.2 数据可视化 / M4.3 整体设计 polish / M4.4 dashboard / M4.5 导出分享 — 这 4 块全在本次 design brief 范围内。

**收到设计 spec 后**:我们会按照 spec 重写前端(SolidJS),不保留任何遗产。如果设计师建议拆 multi-page、嵌入 chart 库、或换交互范式,**只要交互 spec 清晰我们都能实施**。

**联系上下文(可选 deep dive)**:
- `docs/MILESTONES.md §M4` — M4 5 个子 mile 规划
- `docs/dispatches/M4.1.1-tools-redesign.md` — 9 工具的 facts-only 设计哲学
- `docs/dispatches/M4.1-eastmoney-survey.md` — 数据源接口能力清单(辅助决定 chart 数据形态)
- `docs/pm-acceptance/M4.1-stress-resilience.md` — 容错链全景图(影响状态变体设计)
