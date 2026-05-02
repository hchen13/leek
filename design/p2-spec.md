# P2 Spec — Tool Dispatch + Investment-Research Tools

> 本文是 P2 / P3 / P4 三波交付的总结。覆盖 client-side tool dispatch 协议、
> 5 个 client tool + 1 个 server-side tool 的设计与取舍、UI 演进、SSE 事件
> 矩阵。优先权威：本文与代码若有冲突，以代码为准。

依赖：[`p1-spec/agent-loop.md`](p1-spec/agent-loop.md)（agent 主循环）、
[`p1-spec/tools.md`](p1-spec/tools.md)（早期工具 spec，部分被本文超越）、
[`p1-spec/llm-provider.md`](p1-spec/llm-provider.md)。

---

## 1. 总览

P2 把 leek 从"裸 codex 聊天"演进为"多工具协作研究"，加入：

- **Tool dispatch infrastructure** —— client-side function tool 的完整 round-trip
  （SSE function_call 事件解析 → ToolRegistry::dispatch → function_call_output
  灌回下一 turn）。
- **6 个工具协同**：
  | tool | 类型 | 主用 | 数据源 |
  |------|------|------|--------|
  | `web_search` | server-side（codex 内置） | 发现 / 新闻 / 实时事实 | OpenAI 服务端 |
  | `web_fetch` | client-side function | 读全文 | reqwest + dom_smoothie + Jina + strip-tag |
  | `corpus_search` | client-side function | 查 user 自己的 wiki | rust-embed 嵌入 287 篇 .md |
  | `sec_filing_fetch` | client-side function | 美股 SEC 文件 | EDGAR submissions JSON |
  | `tushare_quote` | client-side function | A 股 OHLCV | api.tushare.pro daily |
  | `tradingview_quote` | client-side function | 全球行情快照 | scanner.tradingview.com（内部 API） |
- **UI 三件套**：tool_call chip 流（含完成态、错误态）、events 时间线 panel
  （Cmd/Ctrl+E）、thinking 计时器（▸ Ns / ✓ Ns done）、corpus brain 节点
  点击预览（弹 modal 显示 wikilink 全文）。

x_search (Twitter / X) 已 deferred —— bird 走 GraphQL + cookie auth，
依赖复杂、易碎、ROI 低于 web_fetch + web_search。

---

## 2. Tool dispatch 协议

### 2.1 数据流

OpenAI Responses API 的 function tool 是 multi-turn round-trip。leek 用
`ChatRequest.additional_inputs: Vec<serde_json::Value>` 承载历史 tool
turn 的 `function_call` 和 `function_call_output` 项，重复 `provider.chat()`
直到模型不再发 function_call 或达到 `MAX_TOOL_TURNS = 8`。

```
┌─────────────────────────────────────────────────────────────┐
│  agent::run_chat_reply(...)                                 │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ loop turn = 0..MAX_TOOL_TURNS                          │  │
│  │  ┌─────────────────────────────────────────────────┐   │  │
│  │  │ ChatRequest {                                    │   │  │
│  │  │   messages,            // vault 历史             │   │  │
│  │  │   tools: [WebSearch + Function*],                │   │  │
│  │  │   additional_inputs,   // 上轮 fc + fc_output    │   │  │
│  │  │ }                                                │   │  │
│  │  └────────────────────┬────────────────────────────┘   │  │
│  │                       ▼                                │  │
│  │  provider.chat(req).await                              │  │
│  │                       │                                │  │
│  │  收 LlmEvent stream:                                    │  │
│  │   - TextDelta         → emit agent_message_delta        │  │
│  │   - WebSearchCall     → emit web_search_call            │  │
│  │   - FunctionCall      → push pending_calls + emit       │  │
│  │   - Usage             → emit llm_usage                  │  │
│  │   - MessageEnd        → break inner loop                │  │
│  │                                                          │  │
│  │  if pending_calls.empty()  → break outer loop            │  │
│  │                                                          │  │
│  │  for call in pending_calls {                             │  │
│  │     output = ToolRegistry::dispatch(call.name, args)    │  │
│  │     emit tool_call (status=completed/error)              │  │
│  │     additional_inputs.push(function_call item)          │  │
│  │     additional_inputs.push(function_call_output item)   │  │
│  │  }                                                        │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 SSE 事件类型

| 后端事件类型 | 何时 | 模型层意义 |
|-------------|------|-----------|
| `response.output_text.delta` | 每个 token | TextDelta |
| `response.output_item.added` (item.type=web_search_call, status=in_progress) | 模型决定搜索 | WebSearchCall {in_progress} |
| `response.output_item.done` (item.type=web_search_call, status=completed) | 搜索完成 | WebSearchCall {completed, action} |
| `response.output_item.added` (item.type=function_call) | 工具调用初始（args 待流） | 静默（避免双派发） |
| `response.function_call_arguments.delta` | args 流式 | 静默（done 自带完整 args） |
| `response.output_item.done` (item.type=function_call) | 工具 args 完整 | FunctionCall {call_id, name, arguments} |
| `response.completed` | 整个回复结束 | Usage + MessageEnd |
| `response.failed` | 错误 | Err propagated |

代码：[`crates/gateway/src/llm/openai_responses.rs`](../crates/gateway/src/llm/openai_responses.rs) `parse_one_event`

### 2.3 ToolHandler trait

```rust
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn name(&self) -> &str;
    fn spec(&self) -> ToolSpec;       // 必须返回 ToolSpec::Function {...}
    async fn call(&self, args: serde_json::Value, cancel: CancellationToken) -> Result<String>;
}
```

- `cancel` 来自 agent loop，工具实现应在长 IO 时 `tokio::select!` 它。
- 返回 `Err` **不会**中断 turn —— agent loop 把 `[tool error: ...]` 作为 output
  喂给模型，让模型决定重试 / 放弃 / 换路径。
- `call` 必须线程安全，因为 ToolRegistry 是 `Arc` 共享的。

---

## 3. web_fetch 三级兜底

输入 `{ url, max_chars? }`，输出 markdown。

| Tier | 路径 | 触发 |
|------|------|------|
| Tier 1 | reqwest GET + dom_smoothie Readability + TextMode::Markdown | 默认 |
| Tier 2 | `r.jina.ai/<url>` | Tier 1 markdown < 400 chars |
| Tier 3 | strip-tag（去 script/style/svg/comments + remove `<>` 标签） | Tier 2 网络失败或仍 < 400 |

**关键设计**：所有三级都**保留全文**（无 LLM 二次摘要）。投研场景对原文敏感
（"YoY +12.3% excluding FX impact" 不能丢限定词）。LLM 摘要既费 token 也
有损 —— 不做。

**SSRF 策略**（重要）：
- ✅ 拦 hostname：localhost、`*.local`、`*.internal`、`*.lan`、单 label
- ✅ 拦 IPv4 字面量：169.254.0.0/16（cloud metadata）、100.64.0.0/10（CGNAT）
- ❌ **不拦** 私网范围（10/8、172.16/12、192.168/16、198.18/15、fc00::/7）
  —— clash TUN fake-IP 池就在 198.18/15，自托管 wiki 在 192.168/16；
  把这些也拦了 leek 在大量真实环境会不能工作。
- ❌ **不自己 dns.lookup**：让 reqwest 走系统 resolver，clash 的 fake-IP
  自然转发。

**跨域 redirect**：reqwest custom RedirectPolicy，同源（含 ±www.）跟随，
跨域返 30x 让 caller 拿 Location header 构造 "REDIRECT DETECTED" 提示
让模型重调（仿 claude-code）。

**用户参数**：
- `JINA_API_KEY` env：可选，免费层够 dogfood。
- `LEEK_DISABLE_WEB_SEARCH=1`：诊断用，强制走 client-side dispatch。

代码：[`crates/gateway/src/agent/tools/web_fetch.rs`](../crates/gateway/src/agent/tools/web_fetch.rs)

---

## 4. corpus 接入

### 4.1 编译期：rust-embed

`crates/gateway/src/agent/tools/corpus_search.rs` 用
`#[derive(RustEmbed)] #[folder = "../../corpus"]
#[include = "wikis/**/*.md"] #[include = "sources/**/*.md"]` 嵌入 287 篇
markdown（~7MB）到 binary。`#[features = ["include-exclude"]]` 必需。

启动时一次性 OnceLock 加载到 `Vec<CorpusDoc>`，每个 doc 携带 lowercased
`haystack`（title + tags + body）以加速搜索。

### 4.2 corpus_search

输入 `{ query, tier?, layer?, tags?, limit? }`，关键字 AND 匹配 + filter。
score = 总命中次数；ties by 标题长度（短优先）。

返回每个 hit 的 `{ id, title, tier, layer, tags, score, snippet }` —— snippet
是 240 字符窗口（按 char 边界 UTF-8 安全）围绕首次命中位置。

### 4.3 brain widget 数据流

```
build.rs 兜底 corpus.graph.json placeholder
        ↓
include_str!("../../assets/corpus.graph.json")
        ↓
GET /api/v1/corpus/graph  (axum handler, content-addressed cache 60s)
        ↓
BrainWidget.tsx 异步 fetch → transformBackendGraph()
        ↓
LeekBrain.mount(host, {graph, onNodeClick})
        ↓ user click
GET /api/v1/corpus/doc?id=<wikilink>
  → corpus_search::lookup_doc(id) → CorpusDocView
        ↓
SolidJS modal （title + tags chips + 全文 markdown）
```

实际 graph 重建仍由 CLI `leek corpus rebuild-graph` 触发（避免 build.rs
拆 build-deps crate），build.rs 只兜底确保 file 存在让 include_str! 不失败。

---

## 5. 投研 tools

### 5.1 sec_filing_fetch
- ticker → CIK：`https://www.sec.gov/files/company_tickers.json`，24h 内存
  cache。
- filings 列表：`https://data.sec.gov/submissions/CIK{padded}.json`
- **UA 必须是 SEC 偏好的 `"Name email@domain"` 极简形式**（带括号 / 版本
  marker 会被 403 拦截，已踩坑）。env 覆盖：`LEEK_SEC_USER_AGENT`。
- 输出 metadata + `primary_doc_url` —— 模型自己挑一个再 web_fetch。

### 5.2 tushare_quote
- POST `https://api.tushare.pro` body
  `{api_name, token, params, fields}`，封装 `daily` 接口。
- token 走 `TUSHARE_TOKEN` env，仓库不带。
- ts_code 校验 `<digits>.<SH|SZ|BJ>`。
- 输出 markdown table（date / open / high / low / close / pct_chg / vol）。

### 5.3 tradingview_quote
- POST `https://scanner.tradingview.com/{market}/scan`，body
  `{ symbols: { tickers, query: { types: [] } }, columns }`。
- columns 默认 11 列：name / description / close / change / change_abs /
  volume / market_cap_basic / high / low / open / Recommend.All。
- 无 cookie 即可拿到延迟（~15min）报价；real-time 需要登录 cookie，目前
  不支持。
- 输出 markdown table，volume / market cap 用 k/M/B 单位。

---

## 6. 前端 UI

### 6.1 tool chip 流

每个 tool_call SSE 事件追加到当前 streaming agent message 的
`tool_calls: ToolCall[]`。完成事件按 `call_id` 匹配 in_progress chip
就地更新而不是 append（避免重复）。

显示规则（[`components/LiveChat.tsx`](../frontend/web/src/components/LiveChat.tsx)
`summarizeTool`）：
- `▸ web_fetch: en.wikipedia.org` （进行中，opacity 0.7）
- `✓ web_fetch: en.wikipedia.org` （完成）
- `✗ web_fetch failed: ...` （错误，红色）

### 6.2 thinking 计时器

agent_message_start 起 setInterval 1s，message_end 时把 elapsedSec 冻结到
`LiveMsg.total_sec`，避免流结束后立即显示 "0s"。streaming 时显示
`▸ thinking · 24s`，结束后 `✓ done · 24s`（opacity 0.55 浅化）。

### 6.3 events 时间线 panel

- 后端 `GET /api/v1/sessions/{id}/events?since=&limit=`
  → `vault::events::list_for_session` 返回 EventRow（seq/kind/payload_json/ts）。
- 前端 EventsPanel：抽屉式右滑，Cmd/Ctrl+E 打开，Esc 关闭。
- filter chips: all / agent / tool / user / task / usage / error。
- 点击行展开完整 JSON。
- SSE 实时合流：每个 evtSrc listener 调 `emitTick(e, kind, payload)`，
  通过 `e.lastEventId`（= backend `vault.events.seq`）让 panel dedupe。

### 6.4 brain 节点点击预览

仅在 fixture scenes（A/B/C/D/E）渲染 brain widget，LIVE 模式没有 brain
（设计选择：LIVE 模式专注 chat，brain 是探索性视图）。canvas click
hit-test 半径 10px，命中调 onNodeClick(id, meta) → modal 拉
`/api/v1/corpus/doc` 全文。

---

## 7. 关键决策记录

| # | 决策 | 替代方案 | 选择理由 |
|---|------|----------|---------|
| D1 | web_fetch 不做 LLM 二次摘要 | claude-code 风格 Haiku 摘要 | 投研对原文敏感；leek 用 codex OAuth 也没便宜小模型 |
| D2 | SSRF 不拦私网范围 | openclaw 风格全拦 + opt-in | clash TUN fake-IP 在 198.18/15；强拦在中国用户机器上常态失败 |
| D3 | client tool 错误不杀 turn | 失败即 propagate Err | 让模型决定重试/换路径；与 codex 内置 web_search 行为一致 |
| D4 | corpus 嵌入 binary 而非走 sidecar | 启动时 walkdir | 7MB 可接受；零 IO 启动；无路径配置 |
| D5 | brain 节点 cap=60 by degree | 全部 278 节点 | 340x340 widget 容纳 ~60 个节点不至于挤成雾，更高 degree = 更中心 |
| D6 | x_search (bird) deferred | 现做 | Twitter cookie auth 复杂、易碎、ROI 不如 web_fetch+web_search |
| D7 | events panel seq 来自 SSE lastEventId | client-side counter | backend `vault.events.seq` 是单一事实源，避免 reload 后 dedupe 冲突 |
| D8 | tushare token 走 env 不入 vault | vault.provider_configs 类似 | 简单；tushare token 不算敏感（free tier）；用户机器上配一次就好 |
| D9 | SEC UA 用 `"Name email"` 极简形式 | claude-code 风格带 ua suffix | SEC 服务端解析 UA 偏严，带括号 / 版本号会被 403（已踩坑） |

---

## 8. 已知短板（未来工作）

- 多 LLM provider：anthropic_api_key / openai_api_key 待用户提供 key
  时实施。届时模型 fallback、廉价子任务（routing/清洗）有更好选项。
- corpus_search 是关键字而非 embedding：精确但召回有限。语义搜索靠
  codex 自身理解凑合。embedding 升级要看 dogfood 是否真撞到痛点。
- tradingview 无 cookie：实时报价不准（~15min 延迟）；用户场景如果不是
  日内交易，足够。
- multi-turn loop 顺序执行 tool：fan-out 研究（同时查多个 ticker）目前是
  串行，可优化为并行 tokio::join。

---

## 9. 文件索引

后端：
- [`crates/gateway/src/llm/mod.rs`](../crates/gateway/src/llm/mod.rs) — ChatRequest / ToolSpec / LlmEvent
- [`crates/gateway/src/llm/openai_responses.rs`](../crates/gateway/src/llm/openai_responses.rs) — SSE 解析
- [`crates/gateway/src/agent/mod.rs`](../crates/gateway/src/agent/mod.rs) — multi-turn loop + SYSTEM_PROMPT
- [`crates/gateway/src/agent/tools/`](../crates/gateway/src/agent/tools/) — ToolHandler 实现
- [`crates/gateway/src/api/corpus.rs`](../crates/gateway/src/api/corpus.rs) — graph + doc endpoints
- [`crates/gateway/src/api/sessions.rs`](../crates/gateway/src/api/sessions.rs) — abort + events list

前端：
- [`frontend/web/src/components/LiveChat.tsx`](../frontend/web/src/components/LiveChat.tsx) — chat + tool chip + timer + events drawer trigger
- [`frontend/web/src/components/EventsPanel.tsx`](../frontend/web/src/components/EventsPanel.tsx) — events 时间线
- [`frontend/web/src/components/BrainWidget.tsx`](../frontend/web/src/components/BrainWidget.tsx) — brain wrapper + corpus doc modal
- [`frontend/web/src/corpus-brain.js`](../frontend/web/src/corpus-brain.js) — canvas force-directed graph + click hit-test
