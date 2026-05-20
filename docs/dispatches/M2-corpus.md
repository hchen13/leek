# M2:Corpus 接入 — loader + 工具 + system prompt 注入

> **Dispatch spec(2026-05-20)。** 一次性把 M2.1–M2.4 全做完，让 PM 在前端 e2e 验"agent 调 corpus 搜出原文、cite 出处、按求证纪律答事实"完整链路。中间状态前端没变化、用户没法验，所以合并派活。

## 背景

把 leek 区别于通用 agent 的内容层 corpus 接进来。仓库的 `./corpus/`（用户维护、独立 git 子库、Karpathy LLM Wiki pattern）已经成型：

- 304 个 markdown，8.1MB
- 顶层 `AGENTS.md` / `README.md` / `_meta/` / `sources/` / `wikis/` / `tools/`
- 文件 schema：YAML frontmatter（`title` / `tier` / `layer` / `type` / `format` / `source_url` / `captured`）+ markdown body

M2 把这套 corpus 用起来：**索引 → 工具 → 默认注入主 agent prompt**。

**先读**：
- `docs/ARCHITECTURE.md §4.1`（prompt 顺序）
- `docs/ARCHITECTURE.md §8`（Corpus 设计）
- `corpus/AGENTS.md`（schema 约定）
- `harness/corpus_orientation.md`（M2.3 注入内容）
- `docs/MILESTONES.md` decision log 2026-05-20「求证纪律」（M2.4 设计 sketch）

---

## M2.1 — Corpus loader + BM25 索引

### 模块结构

新增 `crates/gateway/src/corpus/`（`mod.rs` + 必要拆分）：

```rust
pub struct Document {
    pub id: String,
    pub title: String,
    pub frontmatter: HashMap<String, String>,
    pub body: String,
}
pub struct Hit {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub score: f32,
}
pub struct Corpus { /* docs + BM25 inverted index */ }
impl Corpus {
    pub fn load(root: &Path) -> Result<Corpus>;
    pub fn search(&self, query: &str, k: usize) -> Vec<Hit>;
    pub fn read(&self, id: &str) -> Option<&Document>;
}
```

### Loader

递归扫 corpus 根目录下 `*.md`，跳过 `.git/` / `.obsidian/` / `tools/` / 隐藏文件；`_meta/` **纳入**（taxonomy / protocols 也可搜）。每个文件：

- frontmatter：用 `gray_matter` crate 或手切前导 `---`；解析失败 → warn + 空 frontmatter 继续，别因单文件烂掉整 load。
- `id`：从 corpus 根算的 POSIX 相对路径（`sources/principles/munger/daily-journal-2021-meeting.md`）。Windows 反斜杠归一为 `/`。
- `title`：`frontmatter.title` → 第一个 `# heading` → 文件名 stem。
- `body`：frontmatter 之后的全部 markdown。

### BM25 索引

**手写（~80 行）优先**，核心是项目内容层不想被外部 crate 行为意外影响：

- 标准公式，k1=1.2，b=0.75。
- Tokenization：**CJK 字符级 + 英文/数字 whitespace**（用 `unicode-segmentation` 拿 word boundary；中文字符按 char、其他 word lowercased）。不做 stemming / stop words。

### Search 接口

- `search(query, k)`：tokenize + 查倒排 + BM25 打分 + top-k 降序。
- `Hit.snippet`：body 里第一个匹配 token 前后 ~120 字符，UTF-8 边界用 `char_indices` 安全裁。
- 无命中 → `Vec::new()`，不报错。

### `read(id)`

用于 M2.2 的 `corpus_read` 工具；按 `Document.id` 精确查、返回 `&Document`（可拿 frontmatter + body）。

### AppState 注入

`api/mod.rs::AppState` 加 `pub corpus: Arc<Corpus>`；`main.rs` serve 启动前 `Corpus::load`。

### Corpus 根

CLI `--corpus <dir>` > env `LEEK_CORPUS_DIR` > 默认 `./corpus/`。**目录不存在 / 空 → warn + 降级到空 Corpus**（不 abort gateway）；启动日志 `corpus loaded: N docs from <root>`。

### 单测

fixture 在 `crates/gateway/tests/fixtures/corpus/`，3–4 个小 md 中英混排，有/无 frontmatter 各覆盖：

- `load_walks_and_skips_noise`（数量对、`.obsidian/` / `.git/` / `tools/` 不纳入）
- `frontmatter_parsed_or_empty_on_garbage`
- `id_is_posix_relative_path`
- `search_english_token` / `search_cjk_char` / `search_misses_returns_empty`
- `snippet_contains_first_match` + UTF-8 边界不破
- `missing_corpus_dir_degrades_gracefully`
- `read_returns_doc_by_id` / `read_unknown_returns_none`

---

## M2.2 — `corpus_search` + `corpus_read` 工具

两个新工具，走 leek 现有工具契约（同 echo / web_fetch / update_plan）：

- `ToolSpec`（name + description + JSON schema）注册进 tool registry。
- 实现 dispatch 逻辑，从 `AppState.corpus` 拿数据。
- emit `tool_lifecycle` 事件（start / completion / error）。
- 三分输出契约：`model_output` / `display_payload` / `debug_payload`。
- `ToolUi` 注册表加 entry。

### `corpus_search`

- 参数 schema：`{ query: string, limit?: number /* default 5 */ }`
- model_output（给模型读）：
  ```
  Found N hits for "<query>":
  1. <id> — <title>
     <snippet>
  2. ...
  ```
- display_payload（canvas 卡片）：`{ kind: "corpus_search", query, hits: [{id, title, snippet, score}] }`
- debug_payload：`{ query, limit, total_docs, scores: [...] }`

### `corpus_read`

- 参数 schema：`{ id: string }`
- model_output：整个 `body`（可能很长——这是给 agent 推理用的，符合 corpus_read 设计意图）。
- display_payload：`{ kind: "corpus_read", id, title, body_preview: <前 ~400 字>, body_bytes: <length>, frontmatter }`（canvas 默认显示 title + 预览 + 字节数，展开看全文）
- debug_payload：`{ id, frontmatter, body_bytes }`
- id 不存在 → tool error（structured）。

### 前端

`Canvas.tsx` 已有 `renderToolCard`，根据 display_payload 的 `kind` 分支：

- `corpus_search` 卡：`知识库搜索 · "<query>" · N 条`，top N 列表：每行 `<title> · <id-tail>`，点击展开 snippet。
- `corpus_read` 卡：`读取知识库 · <title>`，默认显示 body preview + 字节数；展开看全文（类似 open_page 卡的展开 snippet）。
- 失败卡走现有失败 toggle。

### 单测

- `corpus_search` 单测（用 fixture corpus）：query 有命中 / 无命中、limit 截断。
- `corpus_read` 单测：已知 id 返回 body、未知 id 返回 error。
- `ToolUi` 注册表覆盖测试（同已有 pattern）。

---

## M2.3 — 默认 corpus orientation 注入主 agent system prompt

`crates/gateway/src/agent/prompt.rs::build_system_prompt`：

- 读 `harness/corpus_orientation.md`（`include_str!` 静态嵌入，跟 `identity.md` 同模式）。
- 在 `OPERATING` 与 `# 可用工具` 之间插入。section 头照搬文件里第一行，或加 `# 投研知识库定向` 包一层（看哪个自然）。
- 单测 `prompt_includes_corpus_orientation`：验文件内容出现在 build 结果里。

**Token 上限**：架构目标 < 800 tokens；`harness/corpus_orientation.md` 是用户维护的静态文件，本任务**不做 token 计数 enforce**，只 include。文件超额是内容问题（用户去改），不是工程问题。

---

## M2.4 — 求证纪律 prompt section

`prompt.rs` 在 `OPERATING` 之后、`CORPUS_ORIENTATION` 之前（或两者都在，顺序可调，**只要在工具清单之前即可**）塞一段（直接用 MILESTONES decision log 2026-05-20 锁的措辞）：

```
# 求证纪律

对**具体事实**（股票代码、公司、价格、新闻、事件、日期、数字等），
先搜后答 —— 用 web_search 查到再回答，不要直接用训练里的世界知识。
- 一次搜索返回 0 条或不相关 → 换 query 重试 1-2 次
- 仍搜不到 → 明说"搜不到"
- 若用户仍要训练知识答案 → 显式标注「以下来自训练知识，无法用搜索
  证实，可能过时」再给

分析、判断、推理是你的本职，不必为它做搜索表演。但**分析依赖的事实**
必须搜过、可追溯。
```

写常量在 `prompt.rs` 里（类似 `OPERATING`），不放外部文件 —— 它跟 leek 的 harness 行为强耦合，不应该 corpus-内容化。

单测 `prompt_has_seek_verification_discipline`：验文案里关键短语出现。

---

## Scope 总览（横切）

**新增文件**：
- `crates/gateway/src/corpus/{mod.rs, ...}`（M2.1）
- `crates/gateway/src/agent/tools/{corpus_search.rs, corpus_read.rs}`（M2.2）
- `crates/gateway/tests/fixtures/corpus/*.md`（M2.1 + M2.2 单测）

**改动文件**：
- `crates/gateway/src/api/mod.rs`（AppState 加 corpus，M2.1）
- `crates/gateway/src/main.rs`（serve 启动 load corpus，M2.1）
- `crates/gateway/src/agent/tools/mod.rs`（注册两个新工具 + ToolUi，M2.2）
- `crates/gateway/src/agent/prompt.rs`（corpus_orientation + 求证纪律，M2.3 + M2.4）
- `frontend/web/src/Canvas.tsx`（corpus_search / corpus_read 卡片渲染分支，M2.2）
- `frontend/web/src/types.ts`（若 Artifact 加 corpus 相关字段）
- 必要的 Cargo.toml 加 dep（`gray_matter` / `unicode-segmentation` 等）
- `crates/gateway/src/agent/prompt.rs::tests` 调整

## 不做

- 不引入 embedding（架构决策：先 lexical）
- 不做热加载 / file watcher
- 不做 stemming / stop words / phrase queries
- 不做 corpus-expert subagent（M2.7，独立 milestone）
- 不动 events / transcripts / web_search 已有路径
- 不动 echo / web_fetch / update_plan 工具
- 不引入新事件 kind（corpus_search / corpus_read 都走 tool_lifecycle）

## 验收（PM 浏览器 E2E，本次合并 dispatch 的关键收益）

1. **构建 + 测试** —— `cargo test` / `cargo clippy --all-targets` 全绿；前端 `npm run build` 通过。
2. **启动日志** —— gateway 启动时看到 `corpus loaded: ~300 docs from ./corpus`。
3. **浏览器 E2E**（`LEEK_WEB_SEARCH=1` 起 stack）：
   - 跑一个**纯 corpus 问题**（如 "Munger 怎么看 circle of competence?"）：agent 应当调 `corpus_search` → canvas 出"知识库搜索"卡，top N 命中里包含 munger 目录文件 → 可能继续调 `corpus_read` 读全文 → 最终答案 cite 出处（`<id>`）。
   - 跑一个**会触发求证纪律的事实问题**（如 "$NVDA 现在交易在哪个交易所?"）：agent 应当先调 `web_search`，不再直接用世界知识答。
   - **检查 system prompt 已注入** —— 走 F2 transcript API：`GET /api/v1/sessions/{id}/transcripts/{turn}/{iter}/request | grep -c "求证纪律"` 应 ≥ 1、`| grep -c "corpus"` 应 ≥ 1（orientation 内容里有 corpus 字眼）。
4. **手动 sanity** —— 挑个真 corpus query 直接对着 `Corpus::search` 跑（单测或临时 example），top 5 hits 视觉看着合理。

## 提交

改动留工作区，**别 commit、不 stage**，汇报即可——PM 验收后整个 M2 一次性 commit（**推荐单 commit** 一次到位，本次 dispatch 主题就是"M2 整个落地"）。

---

## 执行建议（meta — 这次 dispatch 偏大，分阶段做更顺）

### Discovery 用 subagent，主 context 留给 coding

这条任务有一段不小的前置 discovery（corpus schema + 现有工具 / 前端模式）。**别让主 context 被研究材料挤掉 coding 预算**。建议派 subagent（`Agent` 工具，`subagent_type: "Explore"` 或 `general-purpose`）做以下独立调研，**一个 message 里并行 spawn**：

1. 读 `corpus/AGENTS.md`，总结"frontmatter 字段、目录结构、loader 要跳过哪些子目录"——返回一页摘要而非整文。
2. 看现有工具实现（`crates/gateway/src/agent/tools/{echo,web_fetch,update_plan,mod}.rs`），返回"`ToolSpec` 怎么注册 / 工具 dispatch 怎么写 / `ToolUi` 注册表对接 / 前端 `Canvas.tsx::renderToolCard` 的 `display_payload.kind` 分支模式"摘要。
3. 读 `harness/corpus_orientation.md`（确认内容 + 长度，给 M2.3 用）。
4. 看现有 fixture 测试模式（`crates/gateway/tests/fixtures/`、`tests/` 目录结构），返回一页 conventions。

这四个 discovery 互相独立，并行 spawn 比串行快、也避免它们污染主 context。

### 分阶段实现

1. **M2.1（Corpus loader + BM25）先做透** —— 这块单测可隔离验证、跟其他模块无依赖。BM25 公式（idf 算、k1/b 边界、tokenize CJK + 英文混排）容易写错，**单测扎实再往下**。`cargo test` 全绿前别开 M2.2。
2. **M2.2（两个工具 + 前端卡片）跟上** —— 依赖 M2.1 的 `Corpus::search` / `read` API。后端工具先写完测过，再动前端 `Canvas.tsx` 加 `display_payload.kind` 分支。
3. **M2.3 + M2.4（两段 system prompt 注入）最后并行做** —— 与前两步独立、改动最小（只动 `prompt.rs`）。

每个阶段做完 `cargo test` / `cargo clippy` 跑一遍，**早失败早改**比累到最后整批修便宜。

### 并行可以做的

- 初始 scaffolding 多个新文件 → 一个 message 里 `Write` 并行。
- `cargo test` / `cargo clippy` / `npm run build` → 一个 background bash 串起来或并发。
- 读多份文件做交叉对照 → 一个 message 多个 `Read`。

### 别走偏

- **不要重构 echo / web_fetch / update_plan** —— 是 reference pattern，照抄结构、不改它们。
- **不要读完整 304 个 corpus 文件** —— schema 从 `corpus/AGENTS.md` + 1–2 个 sample 就够。一旦你开始打开 `sources/principles/munger/*.md` 一个个看，stop——你不需要内容、你需要 schema。
- **不要引入 embedding / 热加载 / phrase queries / corpus-expert subagent** —— 都不在 scope（embedding 是架构决策延后，subagent 是 M2.7 独立 milestone）。
- **不要在 M2.1 就接 AppState** ——loader 模块本身先独立 testable，AppState 注入是 M2.1 末尾的最后一步（一行改）。

### 遇到不确定的工程取舍

spec 倾向哪个走哪个（比如 BM25 手写优先、frontmatter 用 `gray_matter`）。spec 没拍的细节（比如 BM25 的 stopword 列表、snippet 长度精确数）——**挑你认为最稳的、commit message 或代码注释里说明理由即可，别为细节卡死进度**。

### 汇报时附上

报告里除了改动清单 + 测试结果，**也讲清你是怎么分阶段做的、哪几个 discovery subagent 跑过、有没有发现意外的事**（比如 corpus 里有奇怪文件 / 现有 tool pattern 跟 spec 描述不符 / harness/corpus_orientation.md 已经超 800 tokens 等）。这些「过程信息」对 PM 验收 + 后续 dispatch 调整有用。
