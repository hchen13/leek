# M3 — A 股 MVP(5 个核心工具 + 3 个 task 形态)

> **Dispatch spec(2026-05-22)。** MILESTONES §M3 的产品落地。
> **依赖 M2.7 subagent**(并行 task 形态需要),M2.7 完成后启动。

## 背景

L.E.E.K 的产品差异化在投资 corpus + 投研工具。M2 已经接上 corpus,M3 是接上**投研工具的窄而深第一刀**:A 股(MILESTONES "窄优先于深"原则 §6)。

ARCHITECTURE §12 第 1 条 **工具命名中立**:工具名、描述、参数文档、错误消息对厂商中立。具体上游身份(Tushare / 新浪 / 东方财富 / EastMoney 等)只在代码注释 / struct 字段 / env var 名 / `tracing` 日志里出现。

## Scope

### A. 5 个核心 A 股工具(对齐 MILESTONES §M3 列表)

每个工具一份 ToolSpec + 实现 + 单测,加入 `tools::specs()` + `dispatch()`。

#### A.1 `market_quote`

- **签名**:`market_quote(symbol: string) → MarketQuote { symbol, name, price, change, change_pct, volume, turnover, timestamp, ... }`
- **语义**:快照报价
- **数据源**:首选 Tushare `daily` 实时;fallback 新浪 finance hq.sinajs.cn(`hq.sinajs.cn/list=sh600519`)
- **支持的 symbol 格式**:`600519.SH` / `000001.SZ`(标准化大写后缀),或简写 `600519` 自动推断
- 失败处理:vendor 返 stale data 时仍返回带 `data_freshness` 字段;全部 vendor down 时返结构化 error

#### A.2 `get_candlesticks`

- **签名**:`get_candlesticks(symbol, period?: "1d"|"1w"|"1mo", count?: int) → Vec<Candle { date, open, high, low, close, volume, turnover }>`
- **默认**:`period="1d"`,`count=120`(约半年日线)
- **数据源**:Tushare `daily` / `weekly` / `monthly`;fallback 东方财富 K 线接口
- 限制:count ≤ 500;超出返 error 提示 "请缩小时间范围或换 period"
- 单测:fixture vendor response 解析 + 边界 case

#### A.3 `get_financials`

- **签名**:`get_financials(symbol, statement?: "income"|"balance"|"cashflow"|"ratios", period?: "quarter"|"year", count?: int) → FinancialReport`
- **默认**:`statement="ratios"`(综合比率),`period="year"`,`count=5`(5 年)
- **数据源**:Tushare `income` / `balancesheet` / `cashflow` / `fina_indicator`;fallback 公司公告 PDF parse(scope 内 stub,文字说"暂不支持 fallback")
- 返回内容:核心字段(营收 / 净利润 / EPS / ROE / 毛利率 / 资产负债率等)+ raw data 链接
- 命名中立:字段名用中性术语(`revenue` / `net_profit` / `roe` / `debt_to_equity`),不带 vendor 特定 schema 名

#### A.4 `get_company_info`

- **签名**:`get_company_info(symbol) → CompanyInfo { name, industry, market_cap, list_date, business_scope, latest_indicators, ... }`
- **数据源**:Tushare `stock_basic` + `daily_basic`(综合 + 最新指标)
- 必含字段:公司全名 / 行业分类 / 总市值 / 流通市值 / 上市日期 / 业务范围 / 最新 P/E / 最新 P/B / 最新股息率
- 中性:行业分类用中文 / 国标分类,不用 vendor 特定编码

#### A.5 `get_capital_flow`

- **签名**:`get_capital_flow(symbol, period?: "1d"|"5d"|"20d") → CapitalFlow { net_inflow, retail_flow, main_flow, north_flow_available?, ... }`
- **数据源**:Tushare `moneyflow` + `hsgt`(沪股通);north flow 可能不可用(免费 quota 限制)→ 优雅退化,字段 = `null` + `north_flow_available: false`
- 北向资金细分:主力 / 散户;若全部不可用返 partial 数据 + warning

#### A.6 数据 vendor 集成层

新 `crates/gateway/src/vendors/` 目录:

- `tushare.rs`:Tushare HTTP client(token from `LEEK_TUSHARE_TOKEN` env / `~/.leek/config.json::tushare_token`,与 M2.6 settings 系统对齐)
- `sina.rs`:新浪 finance 简单 HTTP(无 token)
- `eastmoney.rs`:东方财富(K 线 fallback)
- `vendor_trait.rs`:每个 vendor impl 一个 trait `VendorQuote` / `VendorCandle` / `VendorFinancial` etc.
- 工具 dispatch 时按优先级 fallback chain

**重要**:vendor 名只出现在 `vendors/` 模块、env var 名、tracing log,**绝不**进 ToolSpec / model_output / display_payload。

### B. 3 个 Task 形态(eval case 端到端)

每个 task 形态对应一份内置 AGENT.md(per M2.7),通过 task 工具委派。

#### B.1 快速扫描("X 现在能不能交易 / 值不值得看?")

- **AGENT 配置**:`harness/agents/quick-screen/AGENT.md`
- **system prompt**:你是快速扫描 worker。父 agent 给你一只股票,你 1-2 工具调用判断"现在能不能交易 / 值不值得看",200-300 字 digest 返回。**不深挖**,只看核心指标。
- **allowed_tools**:`market_quote`, `get_company_info`, `get_capital_flow`
- **预期 wall-clock**:< 2 分钟
- **eval prompt**(主 agent): `"$NVDA 跟 $贵州茅台 现在能不能买?用 task 委派 quick-screen 各跑一次"`
- **预期**:主 agent task 两次,各 < 1 分钟,returns 两份 digest,主 agent 综合给最终判断

#### B.2 深度复盘(完整个股 review)

- **AGENT 配置**:`harness/agents/deep-review/AGENT.md`
- **system prompt**:你是深度复盘 worker。父 agent 给你一只股票,你做完整 review:基本面 + 技术面 + corpus 历史 + 同业对比。15-30 分钟可接受。500-1500 字 digest 返回,带数据引用。
- **allowed_tools**:全集(可调任何工具,典型用 `market_quote`, `get_candlesticks`, `get_financials`, `get_company_info`, `get_capital_flow`, `corpus_search`, `corpus_read`, `web_search`, `web_fetch`)
- **predicted wall-clock**:5-15 分钟
- **eval prompt**:`"深度复盘 600519.SH,用 task 委派 deep-review,然后整理一份对话式投资笔记"`
- **预期**:主 agent task 一次,subagent 自主跑 10-30 iter,canvas 显示 subagent_card 折叠所有内部活动,主 agent 拿 digest 综合

#### B.3 对比(N 个 ticker)

- **AGENT 配置**:`harness/agents/comparison/AGENT.md`
- **system prompt**:你是对比 worker。父 agent 给你一组(N 个)股票 + 维度,你**并行** task quick-screen + corpus-expert 分别取数,然后综合成对比表 + 短结论。
- **allowed_tools**:`task` 自己(嵌套 spawn)+ 综合工具
- **eval prompt**:`"对比贵州茅台 / 五粮液 / 泸州老窖 三家白酒龙头的基本面"`
- **预期**:主 agent task("comparison", 三只票),subagent 内部并行 task("quick-screen", 每只)收数,返回对比表 + 结论

### C. Eval / 测试

#### C.1 单测

- Vendor parser:fixture vendor response → struct 解析(每 vendor 每接口至少 1 个 fixture)
- 工具 ToolSpec schema:validation
- 工具 dispatch happy path / vendor fallback / 全部 vendor down 错误处理

#### C.2 集成测(可选,因依赖网络)

- `tests/m3-integration.rs`(`#[ignore]`,需 LEEK_TUSHARE_TOKEN env): 跑 5 工具各一次拿真实数据,assert schema 合规
- `cargo test --ignored m3_integration` 手动跑

#### C.3 Eval session(产品验收)

新建 `tests/m3-eval-prompts.md`,3 个 task 形态各 3-5 个 prompt(覆盖白酒 / 半导体 / 医药 / 金融 几个行业):
- 快速扫描 × 3-5
- 深度复盘 × 3-5
- 对比 × 3-5

PM 跑 eval session(手动),记录:
- 每 prompt 实际 wall_clock / cost / tool_call_count
- 答案是否 cite 真实数据 / 是否漏掉关键信息 / 是否过度 fabricate
- subagent 行为是否符合 AGENT.md spec(quick-screen 不深挖、deep-review 全面、comparison 真的并行)

### D. 不做(v0)

- **不做美股 / 港股 / 加密**(M3 是 A 股窄而深;M4+ 再说)
- **不做实时推送/订阅**(请求/响应模式)
- **不做交易执行**(只读取,从不下单)
- **不做用户 portfolio 持仓追踪**(memory 是 M5,推迟)
- **不做 K 线图表 UI**(canvas 显示数据 JSON 即可,图表是 future affordance)
- **不做 vendor 自动 API key 申请**(用户手动配 LEEK_TUSHARE_TOKEN)
- **不做付费 vendor 接入**(scope 内只接 free tier;Tushare 免费版 + 公开 API)

### E. 文档同步

- `README.md`:加 A 股工具列表 + Tushare token 配置说明
- `tests/m3-eval-prompts.md`:eval session 用 prompt 集(新文件)

## 验收

### Executor 自测

1. `cargo test --workspace` 全过 + 5 工具单测 + 3 vendor parser 单测
2. `cargo clippy --workspace --all-targets` 0 warning
3. **手测**(需配 LEEK_TUSHARE_TOKEN 或 fallback to 新浪):
   - 启动 gateway,`curl POST /api/v1/sessions/{id}/turns -d '{"input": "$贵州茅台 现在能不能买?用 task 委派 quick-screen"}'` → e2e 跑
   - canvas 看到 subagent_card(quick-screen)+ subagent 内部 tool calls (market_quote, get_company_info, get_capital_flow)
   - subagent 返回 digest,主 agent 综合给最终回答带数据引用

### 汇报里贴

- branch 名 + worktree 路径
- LOC + 涉及文件(预估 15+ 文件)
- vendor 集成 design(每 vendor 哪些 endpoint / fallback chain)
- 5 工具的 ToolSpec definitions(完整 JSON schema)
- 3 个 AGENT.md 内容(quick-screen / deep-review / comparison)
- 手测 e2e 输出 + 截图(canvas subagent_card)
- 任何 design 决策的 rationale(尤其 vendor fallback 策略 / 北向资金不可用怎么退化)

### PM 验收

- 抽查 vendors/ + 工具实现 diff
- 跑 m3-eval-prompts.md 中 3-6 个 prompt(每 task 形态各 1-2)
- 验证命名中立(grep ToolSpec 描述 + display_payload 不应含 "tushare" / "新浪" / "eastmoney" 等 vendor 名)

## 提交

留工作区不 commit,**PM 验收单 commit**(或拆 a/b/c/d:vendors / tools / AGENT.md / eval docs)。

## Open questions(M3 启动时已决策)

| Q | 决策 |
|---|---|
| 哪 5-7 个工具? | 5 个(MILESTONES 列的全套);M4 再扩 |
| Eval 集来源? | 我自己设计 9-15 个 prompt(白酒/半导体/医药/金融/消费几行业) |
| 数据 vendor 选择? | Tushare(主)+ 新浪/东方财富(fallback);用户自配 token |
| 北向资金不可用怎么办? | 优雅退化,字段 null + warning,不阻塞调用 |

## 给 executor 的最后一句

M3 是 leek 第一次有真实业务 vertical。**严守命名中立**(grep "tushare"/"新浪"/"eastmoney" 在 ToolSpec / display_payload 中必须 0 命中)。**工具语义对齐**(field 名用通用术语,不带 vendor schema)。**充分用 subagent 委派**(deep-review 是 M2.7 真正价值的体现)。spec 跟现实对不上 → stop 来报告。
