# M3 A 股 MVP 浏览器手动 eval prompt 集（2026-05-22）

> 起 stack（`LEEK_TUSHARE_TOKEN=<token> LEEK_WEB_SEARCH=1` 起 gateway + 前端 `npm run dev`）后访问 <http://localhost:5173/>，**每条用一个新 session**（左上「+ 新会话」），把 prompt 块复制到底部 composer，回车发送，观察 **chat / canvas / 右栏 Plan widget** 三处反应。
>
> 工具命中预期、subagent 行为是否符合 AGENT.md spec 的判断都标在每条 prompt 下面。
>
> 共 12 条：快速扫描 ×4 + 深度复盘 ×4 + 对比 ×4。

---

## 一、快速扫描（QS1-4）— 走 `task("quick-screen", …)`

> 期望：主 agent task quick-screen 一次或 N 次（multi-ticker 时），每次 <2 分钟，digest 200-300 字，主 agent 综合后给最终答复。canvas 上有 subagent_card，展开能看到 quick-screen 内部的 1-3 个工具调用。

### QS1 · 单只白酒龙头

```
$贵州茅台 现在能不能买？用 task 委派 quick-screen，给我 200 字左右的速判。
```

**预期工具**：主 agent → `task(agent_name="quick-screen", input=...)` × 1；quick-screen subagent → `market_quote("600519.SH")` + `get_company_info("600519.SH")` + (可选) `get_capital_flow("600519.SH")`。**禁止**：subagent 调 `get_financials` / `get_candlesticks` / `web_*` / `corpus_*`（quick-screen 不应深挖）。

### QS2 · 半导体新势力

```
中芯国际（688981）当前估值贵不贵？quick-screen 帮我快速看下。
```

**预期**：科创板 symbol 推断到 SH，subagent 给出 P/E、P/B 与同类对比的快速结论；如果 corpus 没 covered 也要明说。

### QS3 · 双只对比的"前菜"

```
$宁德时代 跟 $比亚迪 现在哪个更值得马上加仓？各 quick-screen 一次然后告诉我。
```

**预期**：主 agent 在同一 message 里**并发**两次 `task("quick-screen", ...)`（同一轮 model output），两个 subagent 并行跑，最后主 agent 综合输出对比结论。canvas 上有两张 subagent_card。

### QS4 · 不存在或冷门 symbol 的优雅退化

```
quick-screen 一下 999999.SH 这只票现在能不能买？
```

**预期**：subagent 在 1-2 次工具调用后 surface 出 "vendor 返回空" 或 "symbol 无效"，digest 里明说"数据不足"，不 fabricate；主 agent 不应继续追加更深工具调用。

---

## 二、深度复盘（DR1-4）— 走 `task("deep-review", …)`

> 期望：主 agent task deep-review 一次，subagent 自主跑 10-30 iter（5-15 分钟），canvas 显示 subagent_card 折叠所有内部活动；digest 500-1500 字，每个数字带 cite。主 agent 拿 digest 整理成更口语化的笔记给用户。

### DR1 · 白酒龙头完整复盘

```
深度复盘 600519.SH 贵州茅台。用 task 委派 deep-review，然后把 subagent 的 digest 整理成一份对话式投资笔记给我，注明哪些是数据、哪些是 leek corpus 的观点。
```

**预期**：subagent 至少调 `get_company_info` + `get_candlesticks` + `get_financials` + `get_capital_flow` + `corpus_search`；digest 里能看到 ROE 趋势、近 6 个月技术面、北向资金态度（如果 quota 开通）、corpus path 引用。最后主 agent 输出的对话笔记应该是中文自然散文，不是表格罗列。

### DR2 · 科创板半导体复盘

```
请深度复盘中芯国际（688981），重点看一下基本面和资金面，最后给 1 段结论。委派 deep-review。
```

**预期**：subagent 应该看出 "近年盈利能力变化" 是核心议题，财报维度 (`get_financials statement="income"` 或 ratios) 至少跑 1-2 次；capital_flow 报告里如果北向不可用应 surface 出来。

### DR3 · 医药白马复盘 + corpus 异议

```
深度复盘恒瑞医药 (600276)，特别关注 leek corpus 里对创新药企的争议性观点。deep-review subagent 跑完后给我 summary。
```

**预期**：subagent 的 corpus_search 必须有 2-3 个 hit；如果 corpus 里有正反两种立场，digest 要显式列出而不是默选一边。

### DR4 · 金融蓝筹复盘

```
深度复盘招商银行（600036）。看基本面 + 估值 + 主力资金。task deep-review。
```

**预期**：subagent 至少跑出 5 年 ROE / 净利润趋势 + 当前估值百分位 + 近 5 日主力资金；digest 里如果北向不可用要清晰标注 `north_flow_available: false`。

---

## 三、对比（CMP1-4）— 走 `task("comparison", …)`

> 期望：主 agent task comparison 一次（input 列出 N 只 + 维度），subagent 内部**并行** task quick-screen + corpus-expert 收数，最终返回对比表 + 短结论。canvas 上有嵌套 subagent_card（comparison 里又开了 N 个 quick-screen）。

### CMP1 · 白酒三龙头

```
对比贵州茅台 / 五粮液 / 泸州老窖 三家白酒龙头的基本面与估值。用 task 委派 comparison。
```

**预期**：comparison subagent 在一轮 model output 里并发 3 次 `task("quick-screen", ...)` + 1 次 `task("corpus-expert", "白酒龙头护城河")`。返回对比表至少含：市值、TTM P/E、ROE、主营、近期态度。

### CMP2 · 半导体三巨头

```
对比中芯国际 / 韦尔股份 (603501) / 北方华创 (002371) 的基本面和盈利能力。comparison 走一下。
```

**预期**：科创板 + 主板 + 创业板三个不同板块都要正确推断 exchange。对比维度应聚焦"盈利能力"，所以财报维度（ROE / 毛利）要在表里。

### CMP3 · 跨行业 2 只对比

```
对比贵州茅台 (600519) 跟招商银行 (600036) —— 我想看看消费白马跟金融白马哪个估值更便宜、防御性更强。comparison。
```

**预期**：comparison 应识别这是跨行业，对比表里要有"行业"列，避免直接用同一财务指标横评（防御性需要不同口径）。

### CMP4 · 超 5 只票的"边界"测试

```
对比贵州茅台 / 五粮液 / 泸州老窖 / 山西汾酒 / 古井贡酒 / 洋河股份 六家白酒。comparison。
```

**预期**：subagent 在 digest 里说"超过对比上限 5 只，建议拆 2 次"，然后**只对前 5 只**做对比；不应硬撑或全部跳过。
