# M0–M3 浏览器手动测试清单(2026-05-22 始,持续追加)

> 起 stack(`LEEK_WEB_SEARCH=1` 起 gateway + 前端 `npm --prefix frontend/web run dev`)后访问 <http://localhost:5173/>,**每条用一个新 session**(左上「+ 新会话」),把 prompt 块复制到底部 composer,回车发送,观察 **chat / canvas / 右栏 widgets** 三处反应。
>
> 由浅入深排列 —— 前面几条不过的话不用跳后面。
>
> 取代 `tests/m1-m2-manual.md`(T1-T9 已 promoted 到这里)。本文件随 milestone 滚动追加。

---

## 第一段:M1 + M2(已验证,从 m1-m2-manual.md 继承)

### T1 · 单轮闲聊(无工具)

**prompt**:

```
你好,简单介绍一下你自己。
```

**预期**:chat 流式回复几秒内完成;canvas 全程空("本会话还没有过程卡片"占位字样);右栏 Plan 也空。

**看点**:turn 段头 `回合 1 · 完成 · 0 工具 · X.Xs`。

---

### T2 · 显式 web_fetch(leek 自家 HTTP 工具)

**prompt**:

```
抓一下 https://example.com 的内容,简短一句话告诉我这是什么页面。
```

**预期**:canvas 出 web_fetch 卡("读取网页 · example.com · HTTP 200 · 字节数"),可点"详情"展开;chat 答 Example Domain 之类。

---

### T3 · 显式 corpus_search(M2.1 + M2.2 主要验证)

**prompt**:

```
用 corpus_search 在知识库里搜 "circle of competence",给我看 top 5 的标题就行。
```

**预期**:canvas 出 **知识库搜索卡** —— `知识库搜索 · circle of competence · N 条结果`,top 6 标题列表,"显示全部 N 条"按钮可展开。chat 列出 top 5 标题。

**看点**:命中里应该有 `wikis/principles/concepts/circle-of-competence.md`、`sources/principles/munger/...md`,BM25 排序合理。

---

### T4 · 显式 corpus_read(M2.2 另一半)

**prompt**:

```
用 corpus_read 读 wikis/principles/concepts/circle-of-competence.md,把核心定义那段念给我。
```

**预期**:canvas 出 **读取知识库卡** —— 标题 "Circle of Competence" + id + 字节数 + frontmatter chips(tier=principles / type=concept / tags / ...) + body preview + "展开全文"按钮。chat 给定义段引用。

---

### T5 · update_plan + 右栏 Plan widget(M1.9 + agent 编排)

**prompt**:

```
我想做个 3 步研究计划:先在 corpus 里查 Buffett 怎么定义 margin of safety,再列 2 个常见误用,最后给一个反例。先把 plan 用 update_plan 写出来再开始。
```

**预期**:**右栏 Plan / TODO widget** 弹出 3 个 step,初始第 1 个 in_progress(▸)、其余 pending;随 agent 推进切换为 ✓ completed。canvas 还会出现 corpus_search 卡。

**看点**:Plan widget 浮在右下角;step 状态随 turn 真的会切。

---

### T6 · 多轮对话(同一 session 接上下文)

**第 1 轮 prompt**:

```
什么是能力圈?去 corpus 里找权威说法。
```

等 turn 完成,**同一 session** 接第 2 轮 prompt:

```
Munger 的版本和 Buffett 的有什么区别?
```

**预期**:第 2 轮 agent **不重新问"你说哪个"**,自动接上第 1 轮的 corpus 上下文继续答(可能再调 corpus_search 找 Buffett 那边)。canvas 出现第 2 个`回合`段,跟第 1 个并列。

**看点**:两个 turn 段都能折叠/展开;chat 上下文连续。

---

### T7 · 求证纪律(M2.4 — 事实问题不能脑补)

**prompt**:

```
$AMD 现在交易在哪个交易所?
```

**预期**:agent **不会**直接用训练知识答"NASDAQ"。它先调 `web_search`(canvas 出"网页搜索"+ 可能 "打开网页" 卡),搜到再答,且答案里带 markdown 链接来源。

**看点**:turn 里至少 1 个 search_lifecycle 事件(canvas 上有"网页搜索"卡);chat 答案末尾带链接。**如果它没搜就答,求证纪律没生效**。

---

### T8 · 长程多工具任务(不指定工具,看 agent 自主编排)

**prompt**:

```
帮我分析一下英伟达(NVDA)这家公司:核心业务是什么、竞争优势在哪、有什么主要风险。先从 corpus 看有没有现成的看法,再用网络查一下最近一个季度的实际经营数据,最后综合给我一段两百字左右的判断。
```

**预期**:turn 较长(30–120s),多工具混合 ——

- `corpus_search` × 1–2(查 corpus 里是否有 NVDA 相关)
- `web_search` × 几条(最新季度财报、风险点)
- 可能 `打开网页`(agent 点开财报原文)
- `update_plan` 拟 3–5 步
- chat 最终答案 cite 出 corpus path + web 链接

**看点**:canvas 按 turn 分段、回合 1 内多张工具卡按时间顺序展开;chat 工具摘要聚合到末尾;点击 chat 工具摘要应该能跳到 canvas 对应卡片高亮。

---

### T9 · 失败工具 + 失败卡 toggle(M1.9 折叠 UI)

**prompt**:

```
抓一下 https://nonexistent-leek-test-xyz.invalid 这个网址。
```

**预期**:web_fetch 失败。canvas 上**默认隐藏失败卡**(turn header 会显示 "X 失败"指示);右上角"**显示失败的工具调用**" toggle 打开后,失败卡显示出来(红 ✗ + 错误信息)。chat 答"抓不到"或类似。

**看点**:toggle on/off 真的控制可见性;失败卡视觉上跟成功卡区分(红边或 ✗)。

---

## 第二段:M2.5 compaction-fix(M1.8 trigger 信号修复 + replay 测试)

### T10 · 缩窗口触发 compaction(机制验证)

**预先**:`LEEK_CONTEXT_WINDOW=10000 LEEK_AUTO_COMPACT_THRESHOLD=0.90` 启 gateway(trigger=9000 tokens)。

**prompt**(单 session 内连发 3 轮):

```
轮 1: 用 corpus_search 帮我把"安全边际"主题下的所有 wiki 全列出来,逐条把标题和路径都念给我。
轮 2: 上面每条 wiki,用 corpus_read 把第一段抽出来给我看。
轮 3: 把上面所有内容整理成一份 600 字的学习笔记。
```

**预期**:跨 3 轮某次 iter 前 `estimate_context_tokens` 跨 9000 → 触发 `auto_compact_lifecycle.completed` event(canvas 显示 "上下文已压缩" 提示);后续 iter 用新 trigger 信号正常继续;assistant 不中断。

**看点**:`compaction_count > 0` 在最终 turn 的 metadata(可通过 `/api/v1/sessions/{id}/transcripts` 查);折叠后 chat 早期 message 替换成 summary placeholder;后续 turn 还能 reference 早期内容(通过 summary)。

---

## 第三段:M2.6 Settings 持久化 + Cost cap UI

### T11 · Settings 页面默认值 + 字段保存

**操作**:
1. 点 sidebar 顶部齿轮 ⚙ 按钮 → Settings modal 弹出
2. 查看默认字段:cost cap = (空,即 0=不限) / idle = 90s / wall = 1800s / max iter = (空) / doom = 3 / auto compact = 0.90 / context window = (空)
3. 填 cost cap = `0.50` USD → 点保存
4. 关 Settings,重启 gateway,再开 Settings
5. cost cap 字段仍是 `0.50`(已持久化到 `~/.leek/config.json`)

**预期**:每字段下方注明 "当前生效:X"(无 env override 时不显示警告);保存成功 toast 提示;`cat ~/.leek/config.json` 看到 `{"cost_cap_usd": 0.5}`。

**看点**:Settings modal 是 overlay 不破坏底下三栏布局;Esc 或点击 backdrop 关闭;响应式宽屏双列 / 窄屏单列。

---

### T12 · 触发 cost cap → 警告条 + 软停

**预先**:T11 已经把 cost cap 设到一个低值(例如 `0.30`,根据 corpus 量调)。

**prompt**:

```
帮我深度复盘贵州茅台(600519.SH):基本面 + 三年财务 + 行业地位 + 估值。要详细,至少 800 字。
```

**预期**:agent 跑几个 iter 调 corpus/web 后 cost 超 `0.30` → backend `turn_cost_capped` event + `assistant_done.stop_reason="cost_cap_exceeded"`;chat 在 assistant message 末尾显示**黄色警告条**:`⚠ 本轮研究达到预算上限 $0.30(实际 $0.XX),已在第 N 步停止。再问一句可以继续 / 或在 Settings 调整预算 [打开 Settings →]`。

**看点**:警告条按钮"打开 Settings →"点击直接弹 Settings modal;已经累积的部分 assistant 答案完整保留(没回滚);用户可以发下一个 user prompt 接力。

---

### T13 · env var override 显示警告

**预先**:`LEEK_COST_CAP_USD=2.5` 启 gateway。

**操作**:打开 Settings 页面,查看 cost cap 字段。

**预期**:cost cap 当前值 = `2.5`,字段旁出现 **"⚠ 被环境变量 override"** badge;input 框本身仍允许编辑(写值会写到 config 文件,但 env 仍 override 实际生效值);提示文字明确"环境变量优先级最高"。

**看点**:`GET /api/v1/settings` 返回 `effective.cost_cap_usd = {value: 2.5, overridden_by_env: true}` + `config.cost_cap_usd = (whatever in file)`。

---

## 第四段:M2.1 Corpus Brain UI(wiki graph + 三层 activation)

### T14 · Corpus Brain tab 打开 + graph 渲染

**操作**:
1. 右栏顶部点 "脑图" tab(从默认 Canvas tab 切过来)
2. graph 显示 ~128 个 wiki 节点 + ~3000 条边(默认 weight slider=3)

**预期**:节点按 force-directed layout 散布,不重叠;principles tier 蓝色 / knowledge tier 绿色或自定义 cluster 色;节点大小由 confidence 编码(high 大,low 小);默认 zoom 适中,可拖动 + scroll 缩放。

**看点**:layout 在 mount 时跑一次 force simulation(收敛 250 iter)后位置 frozen;边数 slider 调到 8 时只看 highest-affinity edges。

---

### T15 · 节点 hover + click

**操作**:
1. hover 单节点 → tooltip 显示 title + path
2. click 节点 → modal 弹出显示 frontmatter chips + 第一段 body preview + "查看完整内容"按钮跳 corpus_read

**预期**:hover tooltip 跟随鼠标;modal 居中,关闭后回到 graph 视图;"查看完整内容"按钮在 chat 发送 `corpus_read <id>` prompt 或直接在 modal 展开全文。

---

### T16 · 三层 activation overlay(关键 — agent corpus usage 可视化)

**操作**:
1. 切回 Canvas tab,发 prompt:`用 corpus_search 找 circle of competence,然后 corpus_read 把第一段拿来`
2. **观察脑图 tab badge**:数字应增加(turn 触发的 wiki 节点数)
3. 切到脑图 tab → 涉及的节点 **live 激活**(pulsing concentric circle)
4. 等 turn 完成 → 节点降级为 **turn**(amber ring)
5. 同一 session 再发一个 user prompt 触发新 turn → 上一 turn 的节点降级为 **session**(0.85 opacity)
6. 切换到 new session → 上一 session 的节点降级为 **historical**(0.55 opacity)
7. 关闭/刷新页面 → historical 节点从 sessionStorage 恢复

**预期**:4 层 activation 视觉清晰区分,降级实时 + 平滑(不需手动 refresh);historical 在 page refresh 后保留。

**看点**:`sessionStorage["leek.brain.historical.v1"]` 是 JSON array;turn 间过渡用 turn_metrics_recorded / assistant_done event 推进 currentTurn id。

---

## 第五段:M2.5 真 Skill / Hook / Plugin(对齐 CC 约定)

### T17 · System prompt 含 "可用 Skills" 索引

**操作**:跑 T1(简单闲聊)之后,通过 `/api/v1/sessions/{id}/transcripts/{turn_id}/1/request` 拉 system prompt(`instructions` 字段)。

**预期**:system prompt 在"可用工具"段之后出现"## 可用 Skills"段,列出 4 个 builtin skill(corpus-research / web-research / equity-valuation / crypto-research),每个一行 description。

**看点**:`disable-model-invocation: true` 的 skill 不进索引;skill 描述 ≤ 1536 chars 截断遵循 CC 规范。

---

### T18 · use_skill 工具加载 skill body

**prompt**:

```
我要做投研,先调用 use_skill 加载 corpus-research,然后按它的指引开始研究"安全边际"主题。
```

**预期**:canvas 出 **use_skill 卡**:`加载 skill: corpus-research · 用户层 / 内置层`,可展开看 body preview;下个 iter 的 input 含 skill body(可在 transcripts 验证);agent 后续行为按 skill 描述执行(先 corpus_search 后 corpus_read)。

**看点**:`use_skill` 工具在 `tools::specs()` 只有 SkillRegistry 非空时才注册;skill body 加载后 turn 内 active(不跨 turn)。

---

### T19 · PreToolUse hook(shell 命令拦截)

**预先**:编辑 `~/.leek/config.json` 加 hook:
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": {"tool_name": "web_fetch"},
        "hooks": [
          {"command": "echo \"$payload\" >> /tmp/leek-hook-test.log", "timeout": 10}
        ]
      }
    ]
  }
}
```

**prompt**:`抓 https://example.com 的内容`

**预期**:web_fetch 工具调用前,hook 命令执行 → `/tmp/leek-hook-test.log` 含一行 JSON(`tool_name=web_fetch`, `session_id`, `hook_event_name=PreToolUse`)。tool 正常完成,chat 答案如常。

**看点**:hook 通过 stdin 喂 payload JSON;exit 0 不 block;timeout 默认 60s 可单 hook 自定义。

---

### T20 · PreToolUse hook block(exit 2)

**预先**:同 T19 但 hook 命令改为 `exit 2`(stderr 加 reason):
```json
{"command": "echo 'web_fetch is policy-blocked in this env' >&2; exit 2", "timeout": 5}
```

**prompt**:`抓 https://example.com 的内容`

**预期**:web_fetch 被 block,不发起 HTTP;canvas 显示 **`tool_blocked_by_hook` event**(红色 ✗ + reason "web_fetch is policy-blocked in this env");chat agent 收到 block 信号后选择换工具或解释"无法抓取"。

**看点**:exit code 2 触发 block;stderr 作 reason 传给主 loop。

---

### T21 · Plugin 加载

**预先**:创建 `~/.leek/plugins/test-plugin/` 目录:
```
.claude-plugin/plugin.json     # {"name":"test-plugin","version":"1.0.0","skills":["sample"]}
skills/sample/SKILL.md         # frontmatter + body
```

**操作**:重启 gateway。

**预期**:启动日志 `M2.5 runtime loaded: 5 skills, 1 plugins`(原 4 + 1);`GET /api/v1/skills` 看到 sample skill(layer 标 user);`GET /api/v1/plugins` 看到 test-plugin 元信息;system prompt 含 sample skill 索引(可 transcripts 验证)。

**看点**:plugin 加载失败不阻塞启动(warn log + skip);plugin 内 skill 注册在 User 层(用户手装 plugin 等同手装 skill)。

---

## 第六段:M2.7 Subagent(等 worker 完成后追加)

T22-T26 占位 — 等 M2.7 worker subagent 实施完成后由 PM 填充具体 case(`task` 工具 + AGENT.md + subagent_card + nested depth + parent_turn_id)。

---

## 第七段:M3 A 股 MVP(等 worker 完成后追加)

T27-T33 占位 — 等 M3 worker subagent 实施完成后由 PM 填充(5 工具 + 3 task 形态 + vendor fallback)。

---

## 跑完之后

- 想看 raw LLM 层(F2 归档):`curl http://localhost:8964/api/v1/sessions/{id}/transcripts` 列出 session 每次 codex 请求的 metadata
- 想 reset:删 vault.db + 删 `~/.leek/config.json`(M2.6 settings),重启 gateway
- 想测 M2.5 compaction-fix replay(不需浏览器):`cargo test --workspace replay`
- M0-M3 全过 → 转 changelog / 跟用户对齐继续 M4+
