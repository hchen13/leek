# M-test-suite — 全量测试 cases(仿 m1-m2-manual.md,覆盖 M0–M3 全部 milestone)

> **Dispatch spec(2026-05-22)。** User 明确要求"测试时仿照 m1-m2-manual.md leek 做一套全量的测试 cases"。
> **这份 dispatch 在 M3 完成后由 PM 或 worker 落地**;现在先把结构 + 已实现部分(T1-T9)固定下来,
> M2.6 / M2.1 / M2.5 / M2.7 / M3 的 case 等对应 milestone 完成后填充。

## 输出物

新文件 `tests/m0-m3-manual.md`(取代旧 `tests/m1-m2-manual.md`),格式严格仿照 m1-m2-manual.md:

- 浏览器手动测试清单
- 每条用新 session
- prompt 块 + 预期 + 看点 三段
- 由浅入深排列
- T1-T40+

## 已实现部分:T1-T9(从 m1-m2-manual.md 继承)

这 9 条已验过,直接迁移到 m0-m3-manual.md,不改:

| # | 主题 | milestone |
|---|---|---|
| T1 | 单轮闲聊(无工具) | M1 |
| T2 | 显式 web_fetch | M1 |
| T3 | 显式 corpus_search | M2.1 + M2.2 |
| T4 | 显式 corpus_read | M2.2 |
| T5 | update_plan + Plan widget | M1.9 |
| T6 | 多轮对话(上下文携带) | M1 |
| T7 | 求证纪律(事实问题不脑补) | M2.4 |
| T8 | 长程多工具自主编排 | M1 + M2 |
| T9 | 失败工具 + 折叠 toggle | M1.9 |

## 待补部分(milestone 完成后填充)

### M2.5 compaction-fix(已落地,但浏览器手动测难触发)
- T10 思路:**用 LEEK_CONTEXT_WINDOW 缩小 + 长 session** 触发 compaction,观察 chat 不中断 +
  `compaction_event` 出现 + 后续 turn 仍能继续 + canvas 显示折叠提示

### M2.6 settings + cost cap(等 worker 落地后补)
- T11 setting 页面打开 + 字段填写 + 保存 + 重启 gateway 验证 config 持久化
- T12 触发 cost cap → 警告条出现 + chat 停在 cap 那一步
- T13 env var override config(`LEEK_COST_CAP_USD=1.0` 启动 → settings 页面显示 "⚠ env override")

### M2.1 Corpus Brain UI(等 worker 落地后补)
- T14 Corpus Brain tab/浮窗显示 + graph 渲染 + hover 节点显示 title + click 节点开 modal
- T15 触发 corpus_search 后看节点实时 "live" 激活 + turn 结束降级 "turn" + 新 turn 后降级 "session"
- T16 多 turn 后历史 "session" → 新 session 进入 "historical" 弱化

### M2.5 真 Skill / Hook / Plugin(等 worker 落地后补)
- T17 system prompt 中"## 可用 Skills" section 出现 + 列出内置 corpus-research / web-research
- T18 use_skill("corpus-research") 工具调用 + skill body 加进下个 iter 上下文 + canvas 显示 skill 卡
- T19 自定义 hook 添加(`~/.leek/config.json` 加 PreToolUse) + 跑 turn → hook 命令执行验证(`/tmp/test.log` 文件检查)
- T20 PreToolUse hook block(exit code != 0) → tool call 跳过 + `tool_blocked_by_hook` event
- T21 plugin 放入 `~/.leek/plugins/<name>/` → 启动后 `/api/v1/skills` 显示该 plugin 的 skill

### M2.7 Subagent(等 worker 落地后补)
- T22 task("general-purpose", "...") → 主 canvas 出 subagent_card + subagent 内部活动折叠
- T23 task("corpus-expert", "...") → corpus_search/corpus_read 在 subagent 内,主 canvas 不平铺
- T24 多重 task 并行调用(subagent → 嵌套 task)→ canvas subagent_card 嵌套显示
- T25 Depth 2 上限 → 第 3 层 task 触发 error
- T26 vault turn_metrics 查 parent_turn_id 关联 + cost 累加(主 + subagent)

### M3 A 股 MVP(等 worker 落地后补)
- T27 单工具 `市场报价 600519.SH` → market_quote 卡 + 报价数据
- T28 单工具 `K线 600519.SH 周线 50 根` → get_candlesticks 卡 + 数据表
- T29 单工具 `财务 600519.SH 年报 5 年` → get_financials 卡
- T30 task("quick-screen", "$贵州茅台") → 1-2 工具内 < 2min digest
- T31 task("deep-review", "复盘 600519.SH") → 多 subagent 嵌套 + corpus + web 综合
- T32 task("comparison", "茅五液 / 五粮液 / 泸州老窖") → 并行子 task + 对比表
- T33 vendor down fallback:停 Tushare 模拟(误用 token) → 自动切新浪 + display warning

## Format 规则(严格)

每个 case:

```markdown
## T<N> · <主题简述>

**prompt**:

```
<复制可执行的 prompt 文本>
```

**预期**:<行为描述,1-3 句>。

**看点**:<最关键的 visual / data assertion>。

---
```

不要写"...如果失败请..."的 verbose troubleshooting(那是 dispatch 该写的)。每个 case 只验"通过/不通过"。

## 验收

PM 跑完所有 T1-T33,在 chat 里逐个标 PASS / FAIL / SKIP(SKIP = 暂未实现的功能,可标 "等 M3 worker 完成")。

## 如何更新

新 milestone 完成 → 新增 T<N> case + 推 m0-m3-manual.md 进 git。**不要删旧 case**(回归 baseline 不丢)。

## 给 executor 的最后一句(如果用 worker 落地)

测试 doc 不写代码,只写测试 case spec。**Format 严格仿 m1-m2-manual.md**,prompt 复制可粘贴的、预期清晰、看点 1 句。

实际跑测试需要浏览器(per memory `e2e_browser_harness.md` 用 Claude-in-Chrome MCP),由 user 决定何时跑。
