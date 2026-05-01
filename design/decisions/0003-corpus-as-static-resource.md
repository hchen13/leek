# ADR 0003 — Corpus 视为 L.E.E.K 的静态资源

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0002](0002-sqlite-vault-single-db.md)（vault 与 corpus 是 hybrid storage）

## Context

Corpus（GitHub 上的 `hchen13/the-corpus`）是 L.E.E.K 的知识库——universal、对任何用户都成立的投资智慧（Buffett / Munger / Dalio 思想 + 概念页 + entity 档案 + sources 原材料）。它是一个**独立维护的 Git 仓库**，由项目所有者通过专门的工具链（手工编排 + 脚本辅助）维护。

**物理放置**：corpus 同时作为两个仓库的 git submodule 存在——

- `~/playground/leek/corpus/`（本仓库）——给 L.E.E.K 运行时消费
- `~/playground/finance-giant/corpus/`（sibling 仓库）——给 corpus 维护工作流使用

两个 working copy 指向同一个 GitHub repo，本质是同一份内容；两套工作流互不干扰。L.E.E.K 默认从仓库内的 `./corpus/` 读取，但 `corpus_path` 配置项可覆盖指向其他 working copy。

之前的设计讨论里（见 handoff §2 + §4），corpus 与 agent 系统的关系曾被描述为：

- agent 在 session 结束后跑 promotion pipeline，输出"哪些值得进 corpus"的候选
- 候选写入 `corpus/inbox/`
- 用户在 Obsidian 里 review → 手动 promote 到 `wikis/`

这个设计假设 agent 系统持续向 corpus **写入候选**——每轮交互都可能触发。

项目所有者在 2026-05-01 设计讨论中明确修订：

> "corpus 的维护是另外去设计的，并不是说每个用户的每次交互都一定要 update corpus。corpus 本来就是 markdown，对于 leek 而言，corpus 就像是项目的静态资源一样。"

这一句把 corpus 在 L.E.E.K 中的角色降级了一档：从"持续被 agent 写入的活资源"变成"静态消费的资源"。

## Decision

**Corpus 在 L.E.E.K 中的角色 = 项目的静态资源**：

1. L.E.E.K 对 corpus 是**只读消费方**（read-only consumer）。所有 agent 工作流不假设 corpus 会被本系统修改。
2. Corpus 的维护是**独立轨道**（手工编排 + corpus 自带工具链 + 项目所有者作为 curator），不与 agent 实时交互绑定。
3. **P1 不实现自动 promotion pipeline**。agent 不会在 session / phase 结束后自动跑"复盘候选"流程。
4. Corpus 默认从仓库内的 git submodule `./corpus/` 读取；`corpus_path` 配置项可覆盖指向其他 working copy（如 sibling `~/playground/finance-giant/corpus/`）。启动时验证存在与可读，**运行时不写**。
5. 跨域引用降级：vault 中的某行 decision / review 可以引用 corpus 路径字符串（如 `["wikis/principles/margin-of-safety.md"]`），渲染时 resolver 按字符串去文件系统读——**软引用，不做双向链表 / 索引重建**。

## P1 不做但接口预留

虽然 P1 不实现 promotion，**架构允许未来实现**：
- agent 仍然有"写 corpus inbox"的协议化通道（一个名为 `submit_corpus_candidate` 的 tool，P1 不注册）
- 该通道的写入路径**只能进 `corpus/inbox/`**，永远不能写 `wikis/` 或 `sources/`——这是 multi-user 安全的 day-1 痕迹

未来要启用 promotion 时，注册这个 tool + 加一个 background loop 定时跑复盘逻辑即可，不破坏 P1 架构。

## Consequences

### Agent 工作流简化

P1 agent loop 只做：理解 → 检索 corpus → 调工具 → 输出投资动作。**不再有"session 结束后跑 promotion"的尾部阶段**——这是从形态层面砍掉的复杂度。

### Corpus 消费形态：检索 + 引用

P1 工具集中两类 corpus 工具：
- **`corpus_search`**：全文 / wikilink / 标签 / 概念图谱遍历的检索
- **`corpus_read`**：按路径或 wikilink ID 读取单篇内容（含 frontmatter）

Agent 在生成 decision / review 时把引用过的 corpus 路径写进 vault 的 `corpus_refs_json` 字段（软引用）。

### Corpus 不进 vault 索引

Corpus 是 markdown 文件，由独立流程维护。**不在 SQLite 里建反向索引**（"哪些 vault 行引用了 corpus 路径 X"）——这种查询足够稀有，临时全表扫描可接受；引入索引带来的同步复杂度不值得。

### 跨域引用是单向软引用

```
vault.decisions[i].corpus_refs_json
  → ["wikis/principles/margin-of-safety.md", "wikis/concepts/owners-earnings.md"]
  → 前端渲染 panel 时 resolver 拿路径去文件系统读
```

不实现的：
- ❌ corpus 反向 link 回 vault（"这条 corpus 概念被多少决策引用过"）
- ❌ 双向 wikilink 解析（vault 里写 `[[margin-of-safety]]` 自动解析）—— vault 用结构化字段，不用 wikilink 语法
- ❌ corpus 文件改动时同步更新 vault 引用（如果 corpus 重命名了文件，老 decision 的 corpus_refs 就失效——接受这个代价，未来可以加 path 重定向表，但不进 P1）

### Corpus 维护保持独立

Corpus 仓库（`hchen13/the-corpus`）的维护流程**不变**：项目所有者在 sibling 仓库 `~/playground/finance-giant/` 用现有的工具链（epub / pdf 转 markdown、手工编排、phase audit）继续工作。L.E.E.K 不参与 corpus 写入。

L.E.E.K 仓库内的 `./corpus/` submodule 是另一个 working copy，仅用于运行时消费——不在这里 commit / push corpus 内容。维护工作仍在 sibling 仓库完成、push 到 GitHub 后，L.E.E.K 这边通过 `git submodule update --remote` 拉最新版本。

## Alternatives Considered

### 自动 promotion pipeline（推迟到非 P1）
- 价值：self-improving loop（"corpus 增长率 = 系统改善率"）是项目的护城河愿景
- 推迟原因：P1 阶段优先打通"用户可以用 leek 做投研"的核心闭环，promotion 是锦上添花
- 重新评估时机：P1 上线 + 用户活跃使用 1-2 个月后，复盘价值是否需要 codify

### Corpus 入库 SQLite 加速检索（被否）
- 全文检索可以靠 SQLite FTS5 或外部 tantivy
- 但 corpus 独立维护、读多写少，文件系统 + 启动期一次性建索引就够了
- 入库会增加同步复杂度（corpus 文件变 → 索引重建）

### Corpus 与 Vault 统一存 SQLite（被否）
- 破坏 corpus 的 "git + Obsidian 编辑" 工作流
- 静态资源 + 动态状态本来就是不同物种，同存反而引入耦合

## 验证标准

- 启动时 corpus_path 验证 < 100ms（含基本一致性检查）
- corpus_search 全文检索 1000 篇文档 p95 < 200ms（启动期建好的索引）
- vault decision 的 corpus_refs 渲染（读取 N 篇引用的 markdown）p95 < 50ms（N ≤ 5）
