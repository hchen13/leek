# M1 + M2 浏览器手动测试清单（2026-05-20）

> 起 stack（`LEEK_WEB_SEARCH=1` 起 gateway + 前端 `npm run dev`）后访问 <http://localhost:5173/>，**每条用一个新 session**（左上「+ 新会话」），把 prompt 块复制到底部 composer，回车发送，观察 **chat / canvas / 右栏 Plan widget** 三处反应。
>
> 由浅入深排列——前面几条不过的话不用跳后面。
>
> 共 9 条（T1 闲聊 → T9 失败工具折叠）。

---

## T1 · 单轮闲聊（无工具）

**prompt**：

```
你好，简单介绍一下你自己。
```

**预期**：chat 流式回复几秒内完成；canvas 全程空（"本会话还没有过程卡片"占位字样）；右栏 Plan 也空。

**看点**：turn 段头 `回合 1 · 完成 · 0 工具 · X.Xs`。

---

## T2 · 显式 web_fetch（leek 自家 HTTP 工具）

**prompt**：

```
抓一下 https://example.com 的内容，简短一句话告诉我这是什么页面。
```

**预期**：canvas 出 web_fetch 卡（"读取网页 · example.com · HTTP 200 · 字节数"），可点"详情"展开；chat 答 Example Domain 之类。

---

## T3 · 显式 corpus_search（M2.1 + M2.2 主要验证）

**prompt**：

```
用 corpus_search 在知识库里搜 "circle of competence"，给我看 top 5 的标题就行。
```

**预期**：canvas 出 **知识库搜索卡** —— `知识库搜索 · circle of competence · N 条结果`，top 6 标题列表，"显示全部 N 条"按钮可展开。chat 列出 top 5 标题。

**看点**：命中里应该有 `wikis/principles/concepts/circle-of-competence.md`、`sources/principles/munger/...md`，BM25 排序合理。

---

## T4 · 显式 corpus_read（M2.2 另一半）

**prompt**：

```
用 corpus_read 读 wikis/principles/concepts/circle-of-competence.md，把核心定义那段念给我。
```

**预期**：canvas 出 **读取知识库卡** —— 标题 "Circle of Competence" + id + 字节数 + frontmatter chips（tier=principles / type=concept / tags / ...）+ body preview + "展开全文"按钮。chat 给定义段引用。

---

## T5 · update_plan + 右栏 Plan widget（M1.9 + agent 编排）

**prompt**：

```
我想做个 3 步研究计划：先在 corpus 里查 Buffett 怎么定义 margin of safety，再列 2 个常见误用，最后给一个反例。先把 plan 用 update_plan 写出来再开始。
```

**预期**：**右栏 Plan / TODO widget** 弹出 3 个 step，初始第 1 个 in_progress（▸）、其余 pending；随 agent 推进切换为 ✓ completed。canvas 还会出现 corpus_search 卡。

**看点**：Plan widget 浮在右下角；step 状态随 turn 真的会切。

---

## T6 · 多轮对话（同一 session 接上下文）

**第 1 轮 prompt**：

```
什么是能力圈？去 corpus 里找权威说法。
```

等 turn 完成，**同一 session** 接第 2 轮 prompt：

```
Munger 的版本和 Buffett 的有什么区别？
```

**预期**：第 2 轮 agent **不重新问"你说哪个"**，自动接上第 1 轮的 corpus 上下文继续答（可能再调 corpus_search 找 Buffett 那边）。canvas 出现第 2 个`回合`段，跟第 1 个并列。

**看点**：两个 turn 段都能折叠/展开；chat 上下文连续。

---

## T7 · 求证纪律（M2.4 — 事实问题不能脑补）

**prompt**：

```
$AMD 现在交易在哪个交易所？
```

**预期**：agent **不会**直接用训练知识答"NASDAQ"。它先调 `web_search`（canvas 出"网页搜索"+ 可能 "打开网页" 卡），搜到再答，且答案里带 markdown 链接来源。

**看点**：turn 里至少 1 个 search_lifecycle 事件（canvas 上有"网页搜索"卡）；chat 答案末尾带链接。**如果它没搜就答，求证纪律没生效**。

---

## T8 · 长程多工具任务（不指定工具，看 agent 自主编排）

**prompt**：

```
帮我分析一下英伟达（NVDA）这家公司：核心业务是什么、竞争优势在哪、有什么主要风险。先从 corpus 看有没有现成的看法，再用网络查一下最近一个季度的实际经营数据，最后综合给我一段两百字左右的判断。
```

**预期**：turn 较长（30–120s），多工具混合 ——

- `corpus_search` × 1–2（查 corpus 里是否有 NVDA 相关）
- `web_search` × 几条（最新季度财报、风险点）
- 可能 `打开网页`（agent 点开财报原文）
- `update_plan` 拟 3–5 步
- chat 最终答案 cite 出 corpus path + web 链接

**看点**：canvas 按 turn 分段、回合 1 内多张工具卡按时间顺序展开；chat 工具摘要聚合到末尾；点击 chat 工具摘要应该能跳到 canvas 对应卡片高亮。

---

## T9 · 失败工具 + 失败卡 toggle（M1.9 折叠 UI）

**prompt**：

```
抓一下 https://nonexistent-leek-test-xyz.invalid 这个网址。
```

**预期**：web_fetch 失败。canvas 上**默认隐藏失败卡**（turn header 会显示 "X 失败"指示）；右上角"**显示失败的工具调用**" toggle 打开后，失败卡显示出来（红 ✗ + 错误信息）。chat 答"抓不到"或类似。

**看点**：toggle on/off 真的控制可见性；失败卡视觉上跟成功卡区分（红边或 ✗）。

---

## 跑完之后

- 想看 raw LLM 层（F2 归档）：`curl http://localhost:8964/api/v1/sessions/{id}/transcripts` 列出该 session 每次 codex 请求的 metadata；`curl .../transcripts/{turn}/{iter}/request` 看 system prompt 和工具 schema；`.../response` 看 codex 返回的完整 SSE。
- 想 reset：直接删 vault.db 重启 gateway（会重建 schema）。
- M1.8 auto-compaction 实测需要把 `LEEK_CONTEXT_WINDOW` 调小（比如 `LEEK_CONTEXT_WINDOW=16000`）再起 gateway，跑几轮研究 turn 就会触发——本清单没收，单独跑。
