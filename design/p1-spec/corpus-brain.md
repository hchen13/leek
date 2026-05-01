# P1 Spec — CorpusBrain Backend

> CorpusBrain 是 L.E.E.K 的 signature 视觉体验——agent 引用 corpus 中的概念时，
> 对应"神经元"被激活脉冲。本文规范后端如何提供 graph 数据 + 触发激活事件。

依赖：
- [`../decisions/0003-corpus-as-static-resource.md`](../decisions/0003-corpus-as-static-resource.md)（corpus 作为静态资源）
- [`../frontend/concept.md`](../frontend/concept.md) §2.1（视觉设计意图）
- [`../frontend/panels.md`](../frontend/panels.md) §3（widget 视觉规约）
- corpus 仓库 [`AGENTS.md`](../../corpus/AGENTS.md)（frontmatter / wikilink 约定）

前端实现：[`frontend/web/src/corpus-brain.js`](../../frontend/web/src/corpus-brain.js)

## 1. Graph 数据来源：Build-time 静态 JSON

corpus 是 git submodule（`hchen13/the-corpus`，Obsidian vault）；leek 把它当作**静态资源**对待（[ADR-0003](../decisions/0003-corpus-as-static-resource.md)）。所以 graph 是 **build-time 派生**的产物，而不是 runtime 扫描。

### 1.1 Build pipeline

```
corpus/                                    crate::corpus::build
├── wikis/principles/...                          ↓
├── wikis/knowledge/...           ──→     扫描 markdown
├── sources/principles/...                parse YAML frontmatter
├── sources/knowledge/...                 parse [[wikilinks]]
└── _meta/index.md (excluded)                     ↓
                                          生成 corpus.graph.json
                                          （embed 到 binary）
```

执行时机：

- `cargo build` 时通过 `build.rs` 触发
- 或独立命令：`leek corpus rebuild-graph`
- 输出 → `crates/gateway/assets/corpus.graph.json`，由 `include_bytes!` 编译进 binary

corpus 文件改动 → 重新 `cargo build`。**不做 hot-reload**——P1 simplicity。

### 1.2 扫描规则

scan corpus/ 目录下：

| 路径 | 是否纳入 graph |
|--|--|
| `wikis/principles/**/*.md`   | ✅ 节点 |
| `wikis/knowledge/**/*.md`    | ✅ 节点 |
| `sources/principles/**/*.md` | ✅ 节点 |
| `sources/knowledge/**/*.md`  | ✅ 节点 |
| `_meta/index.md`             | ❌（catalog 排除）|
| `_meta/log.md` / `status.md` / `taxonomy.md` | ❌ |
| `_meta/protocols/**/*.md`    | ❌（spec 文档不是 corpus）|
| `tools/*`                    | ❌（脚本）|
| `AGENTS.md` / `README.md`    | ❌（meta）|

每个保留的 markdown 文件 = 一个 node。

## 2. Node schema

```typescript
interface CorpusNode {
  id: string;          // 路径标准化的 wikilink，如 "wikis/principles/concepts/margin-of-safety"
  cluster: NodeCluster;
  title: string;       // 来自 frontmatter `title`
  slug: string;        // 来自 frontmatter `slug`（与 filename 一致）
  type?: string;       // frontmatter `type`：entity | concept | topic | comparison | query | letter | ...
  tier: "principles" | "knowledge";  // frontmatter `tier`
  layer: "wikis" | "sources";        // frontmatter `layer`
  tags: string[];      // frontmatter `tags`
  degree: number;      // 入度+出度（build 时计算）
}

type NodeCluster =
  | "principles-wikis"
  | "principles-sources"
  | "knowledge-wikis"
  | "knowledge-sources";
```

`cluster` 由 path 决定：`{tier}-{layer}`——4 cluster 与 corpus-brain.js 已实现的 4 类节点配色直接对应。

节点视觉大小由前端 `clamp(degree / max_degree, 0.4, 1.0) * baseRadius` 派生，后端不预先算 size。

## 3. Edge schema

```typescript
interface CorpusEdge {
  from: string;        // node id
  to: string;          // node id
  kind: EdgeKind;
}

type EdgeKind =
  | "wikilink"         // body 中 [[link]]
  | "source-ref";      // frontmatter `sources:` 列出的引用
```

### 3.1 边的来源

- **wikilink 边**：parse markdown body 中所有 `[[...]]`（corpus 不用 alias 形式，详见 corpus AGENTS.md "Wikilink placement discipline"）
- **source-ref 边**：parse YAML frontmatter `sources: [...]`

### 3.2 跨 tier 约束

corpus AGENTS.md 规定：

- `wikis/knowledge/` → `wikis/principles/` ✅ 允许
- `wikis/principles/` → `wikis/knowledge/` ❌ 禁止（lint 检查）

build 时检测到禁止的 wikilink，发 build warning（不阻断）但仍生成边——graph 视觉是 truth；规则违规不应被 graph 隐藏（用户能从异常的边看出问题）。

## 4. corpus.graph.json 形态

```json
{
  "version": 1,
  "generated_at": "2026-05-01T12:00:00Z",
  "corpus_commit": "abc1234",
  "nodes": [
    {
      "id": "wikis/principles/concepts/margin-of-safety",
      "cluster": "principles-wikis",
      "title": "Margin of Safety",
      "slug": "margin-of-safety",
      "type": "concept",
      "tier": "principles",
      "layer": "wikis",
      "tags": ["buffett", "graham"],
      "degree": 14
    }
  ],
  "edges": [
    {
      "from": "wikis/principles/concepts/margin-of-safety",
      "to": "wikis/principles/concepts/circle-of-competence",
      "kind": "wikilink"
    }
  ]
}
```

文件大小预估：~600 节点 × 200 bytes + ~2000 边 × 80 bytes ≈ 280 KB。gzip 后 ~60 KB。embed 到 binary 完全可接受。

## 5. Activation 协议

### 5.1 三种 intensity

agent 在分析过程中"触碰" corpus 节点时触发激活：

| Intensity | 触发条件 | 视觉效果（详见 panels.md §3） |
|--|--|--|
| `search_hit` | `corpus.search` 返回的命中（agent 还没深入读） | 节点浅色脉冲一次（~150ms） |
| `deep_read`  | `corpus.read` 返回的全文（agent 读了内容） | 节点强色 + 1s 持续 |
| `cited`      | 节点 path 出现在 deliverable.corpus_refs 中 | 节点变核心色 + 边缘 ripple（~500ms） |

### 5.2 事件 payload

```typescript
interface CorpusNodeActivatedEvent {
  kind: "corpus_node_activated";
  payload: {
    wikilink_id: string;             // = CorpusNode.id
    intensity: "search_hit" | "deep_read" | "cited";
    trigger_tool_call_id?: string;   // 关联 tool call run_id
    trigger_subagent_run_id?: string; // 如果由 subagent 触发
    ts: string;                      // ISO8601
  };
}
```

### 5.3 触发位置

main agent loop 在以下时机自动 emit（agent 不需要主动调"激活"工具）：

| Loop 事件 | 激活强度 | 备注 |
|--|--|--|
| `corpus.search` 完成、命中节点 | `search_hit`（每个命中一次） | hit list 中所有节点都触发 |
| `corpus.read` 完成、读取节点   | `deep_read` | 单节点 |
| `decision.draft` 完成，含 corpus_refs | `cited`（每个 ref 一次） | 决策 deliverable 引用 |
| `review.draft` 完成，含 corpus_refs | `cited` | 复盘 deliverable 引用 |
| subagent 内部触发的 corpus tool | 同上，但 payload 的 `trigger_subagent_run_id` 字段填充 | subagent 工具调用同样触发激活 |

### 5.4 多次激活的合并

同一节点在短时间内（< 200ms）多次触发**同 intensity** → 合并为一次（防止 search 命中 50 个节点导致前端 flood）。

但 intensity **升级不合并**：

- `search_hit` → `deep_read`：发两次（第二次升级动效）
- `cited` 永远独立发（决策最终引用是仪式性时刻，不能合并）

合并在 emitter 层做：维护一个 `(wikilink_id, intensity) → last_emit_ts` 的内存 map，过滤重复。

## 6. 持久化层

### 6.1 不存 graph 数据到 vault

graph 在 binary 内 embedded（build-time）；`/api/v1/corpus/graph` 直接返回内置数据。
不同 user 共用同一份 graph（corpus 不分 per-user）。

### 6.2 激活历史 → vault.events

每次 `corpus_node_activated` 事件按通用 events 表持久化（[`data-schema.md`](data-schema.md) §2.2）——不另开表。这给前端 "节点的近期激活历史" 功能（[`../frontend/concept.md`](../frontend/concept.md) §4.4 Browse Step 3）提供数据源。

查询节点的近期激活历史：

```sql
SELECT payload_json, ts FROM events
WHERE user_id = ?
  AND kind = 'corpus_node_activated'
  AND json_extract(payload_json, '$.wikilink_id') = ?
ORDER BY ts DESC LIMIT 20;
```

P1 简化：不建 corpus 反向索引（events 表的 json_extract 全表扫描可接受，因为这种查询稀有）。

## 7. API endpoints

详见 [`api.md`](api.md) §4.10：

```
GET /api/v1/corpus/graph
Response: 完整的 corpus.graph.json（含 nodes + edges + metadata）

GET /api/v1/corpus/search
GET /api/v1/corpus/read
POST /api/v1/corpus/reload   # 触发 rebuild graph（开发期方便用）
```

graph endpoint 设 `Cache-Control: max-age=86400`（graph 是 build artifact，不变）。

## 8. 实施 checklist

### 后端

- [ ] `crate::corpus::build`：扫描 corpus/ 目录、parse frontmatter（`serde_yaml`）、parse wikilinks（`pulldown-cmark` + regex）
- [ ] `corpus.graph.json` schema 序列化（与本文 §4 严格对齐）
- [ ] `build.rs` 在 cargo build 时自动 regenerate graph（依赖 `corpus/` 目录的 mtime）
- [ ] CLI 命令：`leek corpus rebuild-graph`（手动触发）
- [ ] `CorpusActivationEmitter`：在 corpus.search / corpus.read / decision.draft / review.draft 完成时自动 emit
- [ ] 200ms 合并 dedup（同节点同 intensity）
- [ ] events 持久化（沿用通用 events 表）
- [ ] `/api/v1/corpus/graph` endpoint serve embedded JSON
- [ ] `/api/v1/corpus/reload` endpoint 触发 hot rebuild（开发期）

### 前端

- [x] `corpus-brain.js` 渲染（vanilla canvas force graph，已 done with hardcoded 60 nodes）
- [ ] 启动时 fetch `/api/v1/corpus/graph` 替换 hardcoded fixture
- [ ] 订阅 SSE `corpus_node_activated` event → 调用 `LeekBrain.fire(wikilink_id, intensity)`
- [ ] 节点点击 popover：拉 `corpus.read` 显示全文 + 近期激活历史

### 测试

- [ ] build pipeline：在 corpus 实际仓库跑一次 build，验证 graph 节点数 / 边数 / cluster 分布合理
- [ ] frontmatter parser：missing/malformed frontmatter 优雅降级（fallback title = filename）
- [ ] 跨 tier 违规检测：构造一个测试 case，验证 build warning 但 graph 仍生成
- [ ] dedup 测试：构造 50 个 search hit 同时到达，验证前端只看到合并后的事件
- [ ] e2e：触发一次 `corpus.search`，验证前端对应节点正确激活脉冲
