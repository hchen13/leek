# M2 follow-up:删 echo 工具 + 全局 markdown 实时渲染

> **Dispatch spec（2026-05-20）。** M2 接入落地之后两件 polish 一并做：
>
> - **A** 删 M0 留下的 `echo` 测试工具（线上无用、对 model 是噪声）
> - **B** chat 和 canvas 全局 markdown 渲染，chat 流式输出**实时渲染**（每个 `assistant_delta` 都重新解析，不是等流完才渲染）

## 背景

- `echo` 是 M0 时期为了验证 `function_call → function_call_output` 链做的最小工具。今天有 `web_fetch` / `corpus_search` / `corpus_read` / `web_search` / `update_plan`，echo 既无验证价值，也会在 spec 里诱导 model 在 testing 场景误调。**删干净**。
- chat 当前对 assistant text 是纯文本渲染：`[Nasdaq](https://…)` / `**重点**` / 三反引号代码块全按 raw 出现；canvas 上 corpus_read body / note_trace / open_page snippet 同样是 raw 文本。这两边都要走 markdown。
- chat assistant 是流式的（一串 `assistant_delta` 累加进 `streaming.text`），渲染要在每个 delta 都重新 parse，**不能**等 `message_created` 落地才一次性 render。

---

## Scope

### A. 删 echo

**代码（删尽，active 路径里 `echo` 应 ≈ 0 引用）：**

- 删文件 `crates/gateway/src/agent/tools/echo.rs`
- `crates/gateway/src/agent/tools/mod.rs`：删 `mod echo;`、`specs()` 里 `echo::spec()`、`ui()` 里 `"echo" => Some(echo::ui())`、`dispatch()` 里 `"echo" => echo::run(args)`、`ui_registry_covers_the_tool_set` 测试里 echo 一行
- 单测 fixture 里所有 `"echo"` 字面量改成 `"web_fetch"`（无副作用替换）：
  - `crates/gateway/src/agent/guards.rs::tests` doom-loop fixtures
  - `crates/gateway/src/agent/events.rs::tests` canvas_identity (`echo::{}` → `web_fetch::{}` 或类似)
  - `crates/gateway/src/agent/prompt.rs::tests` ToolSpec fixture
  - `crates/gateway/src/agent/compaction.rs::tests` function_call fixture
  - `crates/gateway/src/llm/responses.rs::tests` 多处 echo JSON fixture
- doc 注释更新：
  - `crates/gateway/src/agent/tools/mod.rs` 头部 "M1 kept the surface tiny (echo, web_fetch)" → 删 echo
  - `crates/gateway/src/agent/tools/web_fetch.rs` 头部 "A real, non-deterministic tool alongside `echo`" → 改

**保留**（讲历史，不是 active 引用）：

- `crates/gateway/src/main.rs` 头部注释 "M0's echo worker" —— 这是讲 M0 echo worker 演化路径，留着。
- `crates/gateway/src/llm/codex.rs` "echo the Authorization header" —— "echo" 是动词不是工具名，留着。

**Docs**：

- `README.md`（英 + 中两段）工具列表删 echo。
- `tests/m1-m2-manual.md`：**PM 已预先 clean** —— T2 删除、T3-T10 已 renumber 为 T2-T9（commit 见 git log）。worker 不动这个文件。
- `docs/MILESTONES.md` 里 M1 scope 表格 "M1.x | turn_metrics 表 + GuardConfig 脚手架" 等行没提 echo，不用动；如果其它地方有 echo 字眼一并清。

### B. 全局 markdown 渲染

**依赖**：

- `frontend/web/package.json` 加 `marked` + `dompurify`（约 50 KB 增量，可接受）。`npm install` + 跑 build 验证。
- **不要**引 `unified` / `remark` / `shiki` / `highlight.js` 等重生态。MVP 纯净 markdown。

**统一入口**：

新增 `frontend/web/src/markdown.ts`：

```ts
import { marked } from "marked";
import DOMPurify from "dompurify";

marked.setOptions({ gfm: true, breaks: true });

export function renderMarkdown(text: string): string {
  return DOMPurify.sanitize(marked.parse(text ?? "", { async: false }) as string);
}
```

所有 markdown 渲染**只过这一个函数**，便于以后改配置 / 加 hook（比如给 a 加 `target="_blank" rel="noreferrer"`，可用 DOMPurify hook 实现）。

**Chat 渲染（关键 — streaming 实时性）**：

- `Chat.tsx`（或对应组件）：
  - **历史 message**（`Message[]` 里 role==assistant）的 content：JSX `<div innerHTML={renderMarkdown(m.content)} />`。
  - **streaming 中的 assistant**（`wb.state.streaming.text`）：JSX `<div innerHTML={renderMarkdown(streaming().text)} />`——**Solid 的 fine-grained 反应式会让这个 binding 在每次 `streaming.text` 变化时自动重 parse + 重 render**。这就是"实时渲染"。**别**把 markdown 渲染放到 `message_created` 后，那等于 batching 到流末尾。
  - USER bubble 也走同一个 `renderMarkdown`（用户输入若是 markdown 也渲染）。
- 渲染失败 / 空字符串 → 输出空字符串，不抛错。

**Canvas 渲染**：

`Canvas.tsx` 这些字段过 `renderMarkdown`：

| 卡片 | 字段 |
|---|---|
| note 卡（note_trace） | `art.text` |
| corpus_read | `body_preview` 和展开后的 `body` |
| corpus_search | 每条 hit 的 `snippet` |
| open_page（web_search action） | `snippet` |
| find_in_page（web_search action） | 每条 `match` |
| search 卡 result snippet | 如果 result 有 `snippet` 字段（M1.9.4 web_search 暂没,但保留 hook） |

**保留 plain text 不走 markdown**：

- 工具 error message
- url / host / id 等元数据字段
- frontmatter chips（已经是 key:value 结构）
- 标题、字节数等数字标识

**Plan widget（右栏）**：

- step text 走 markdown（短文本，加粗 / inline code 这类常见）。

**CSS**：

- 新增 `.markdown-body`（或 `.md`） className，统一 typography:
  - `h1-h6` 字号阶梯
  - `ul / ol` 缩进 + bullet
  - `blockquote` 左边框 + muted 色
  - `code` inline:等宽字体 + 浅底色（暗主题用 `#1f1f1f` 或 `#222` 之类，别刺眼）
  - `pre code` 块:暗底 + 横向滚动 + 不 wrap
  - `a` 链接颜色匹配现有主题 link 蓝
  - `strong / em` 加粗 / 斜体
  - `table` 简单 border + 内边距（GFM 用）
  - `hr` 灰线
- 现有 `.card-pre` / `.corpus-snippet` 等 class：**仔细检查是否与 markdown 渲染的 `<pre><code>` 冲突**（比如 white-space 规则）。冲突就把现有 class 重命名或并入 `.markdown-body`。

**XSS 防护**：

- 每次渲染**都过 DOMPurify**——LLM 输出本质上是 untrusted content（prompt injection 可能注入 `<script>` / `onerror`），sanitize 必须做。
- DOMPurify 默认配置已经足够安全（剥所有 `<script>` / `javascript:` URL / event handlers）。

**未闭合 markdown 流式行为**：

- streaming 中出现三反引号还没闭合 → marked 把它当 raw 处理直到 EOF。这是**可接受**的"逐字进入"体验，**不要**为了完美闭合加特殊状态机。

### 不做

- 不上代码块语法高亮（highlight.js / shiki 都不上）
- 不上 math 渲染（katex / mathjax 都不上）
- 不动 events / transcripts / 工具 dispatch / store.ts 数据结构（只动 render 层）
- 不引入 SolidJS 之外的 framework
- 不改 chat optimistic UI 那套

---

## 验收

### Executor 自测（汇报前都做完）

1. **构建测试**：`cargo test` / `cargo clippy --all-targets` 全绿（echo 删完所有 fixture 单测得改对）；前端 `npm --prefix frontend/web run build` 通过；记下 bundle 增量（应 ~50 KB）。
2. **echo grep 自检**：
   ```sh
   grep -rn '\becho\b' crates/ frontend/web/src/ tests/ \
       --include='*.rs' --include='*.ts' --include='*.tsx' --include='*.md' \
       | grep -v 'main.rs.*M0' | grep -v 'codex.rs.*Authorization'
   ```
   预期 ≈ 0 行命中。
3. **markdown 渲染 API 层 sanity**：
   - 起 gateway，新 session 发 `用 corpus_read 读 wikis/principles/concepts/circle-of-competence.md`，curl `GET /api/v1/sessions/{id}/transcripts/{turn}/{iter}/response` 拿到响应流，确认响应包含 markdown 内容；然后 `GET /api/v1/sessions/{id}/messages` 看 assistant message 是 markdown 文本（未渲染的）——后端无变化。
4. **markdown 浏览器 sanity（执行 session 没浏览器，但能跑前端 unit 思路代替）**：写一个最小测试或用 `node -e` 验 `renderMarkdown("**bold** [link](https://x)")` 输出包含 `<strong>bold</strong>` 和 `<a href="https://x"...>`。同时 sanitize 测试：`renderMarkdown("<script>alert(1)</script>")` 输出**不含** `<script>`。

汇报里贴这几条的命令 + 输出片段。

### PM 验收（浏览器视觉 + 实时性 — PM 自己做）

- 跑 `tests/m1-m2-manual.md` 调整后的全部测试，每条都关注:
  - chat 流式中 `[link](url)` 是否**实时**就出 `<a>`（不是等流完才变样）—— 可拖慢 Chrome devtools network 或视觉观察
  - canvas corpus_read 卡的 body 里 markdown 标题、列表、inline code 是否格式化
  - 暗色主题下 markdown 元素（链接颜色、code 块底色、blockquote 边框）视觉舒适
  - 失败工具 error message 仍是 plain text，没被 markdown 解析坏

## 提交

改动留工作区，**别 commit、不 stage**，汇报即可——PM 验收后单 commit。

---

## 执行建议（meta）

- **不用 subagent discovery** —— 这条比 M2 小，删 echo + markdown 集成都是窄面手术。
- **可分两段并行**：先把 echo 删完跑 `cargo test` 绿，再上 markdown。两边互不依赖。
- **markdown lib 别走偏**：`marked` + `dompurify` 稳定组合，别引 SSR-only / heavy 生态。
- **streaming 渲染陷阱**：Solid `<div innerHTML={() => renderMarkdown(streaming().text)} />` 这种 accessor 写法。**不要**用 `createMemo`（每次 parse 也无所谓，性能不是瓶颈，避免引入依赖追踪坑）；**不要**在 effect 里手动 setInnerHTML（容易脱出反应式追踪）。
- **未闭合 markdown 流式**：marked 把未闭合 ``` 当 raw 处理直到 EOF —— 是可接受的逐字进入体验，别为完美加状态机。
- **DOMPurify 必须每次调用**：LLM 输出是 untrusted，sanitize 不能省。
- **CSS 整体迁移**：检查现有 `.card-pre` / `.corpus-snippet` 类是否跟 `.markdown-body pre code` 冲突，必要时改名或并入。
- **汇报附上过程信息**：哪些 echo 引用挪到了哪儿、markdown 处理了哪几个渲染点、bundle 增量、有没有视觉警报、`renderMarkdown` 单测怎么写的。
