# ADR 0002 — Vault 用 SQLite 单库多 user_id

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0003](0003-corpus-as-static-resource.md)（确定 hybrid storage 形态）、[0009](0009-portfolio-as-research-context.md)（portfolio 是 vault 的一类视图）

## Context

L.E.E.K 的 per-user 运行时状态包括：
- Sessions（对话 session 元数据）
- Messages（user / agent 消息历史）
- Decisions（决策草稿与已确认决策）
- Holdings（portfolio：持仓快照，作为 agent 投研参考）
- Reviews（复盘记录）
- Mandates（用户的投资准则 / 风险偏好）
- Watchlists（自选股）
- Artifacts（agent 生成的中间产物：图表、文档、计算结果）

这些数据是结构化的、需要查询（"我去年这只股票的所有决策"、"距离最近 30 天有 review schedule 的决策"），并且**写入频率高**（每条 agent 消息都要落库）。

候选方案：

| 方案 | 优势 | 劣势 |
|--|--|--|
| **文件系统（markdown + frontmatter）** | Obsidian 直接编辑；与 corpus 心智一致 | 查询能力差（只能 grep）；并发写入容易冲突；schema 演化只能靠规约 |
| **SQLite 单库 / 单文件 / 多 user_id 列** | 单文件备份；查询能力强；schema migration 工具成熟（sqlx）；横扩 Postgres 几乎零改动 | 不能 Obsidian 直接编辑；序列化大段文本要用 TEXT 字段 |
| **SQLite 多文件 per-user** | 隔离强 | 多文件开 / 切换 / cross-user 查询复杂；与 cloud Postgres 模型不一致 |
| **Postgres 直接起步** | 一步到位 | P1 单机部署增加依赖；本地用还要装 Postgres |

## Decision

**Vault = SQLite，单库多 user_id 列**。

- 本地：`~/.leek/vault.db`
- Cloud：同 schema 直接迁到 PostgreSQL（driver 切换，schema 几乎零改动）
- 隔离方式：每张表第一列都是 `user_id TEXT NOT NULL`，所有查询带 `WHERE user_id = ?`

## Schema 概要（详细 schema 见 `p1-spec/data-schema.md`）

```sql
-- 用户档案（仅元数据，敏感凭证另存）
CREATE TABLE users (
    user_id TEXT PRIMARY KEY,
    display_name TEXT,
    created_at TEXT NOT NULL
);

-- 投资准则
CREATE TABLE mandates (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    spec_json TEXT NOT NULL,    -- 整套准则的 JSON
    active INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

-- Sessions
CREATE TABLE sessions (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,            -- UUID
    title TEXT,
    created_at TEXT NOT NULL,
    last_active_at TEXT NOT NULL,
    closed_at TEXT,
    PRIMARY KEY (user_id, id)
);

-- Messages（user / agent 多轮）
CREATE TABLE messages (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,          -- user | agent | tool | system
    content_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, session_id, seq)
);

-- Decisions（投资动作）
CREATE TABLE decisions (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT,             -- 来源 session（可空：手工录入）
    ticker TEXT NOT NULL,
    direction TEXT NOT NULL,     -- long | short | close | adjust
    size_pct REAL,
    stop_loss REAL,
    target REAL,
    horizon_days INTEGER,
    rationale TEXT NOT NULL,     -- markdown 长文
    corpus_refs_json TEXT,       -- ["wikis/principles/margin-of-safety.md", ...]
    review_schedule_json TEXT,   -- ["2026-06-01", "2026-08-01"]
    status TEXT NOT NULL,        -- draft | confirmed | closed | superseded
    created_at TEXT NOT NULL,
    confirmed_at TEXT,
    closed_at TEXT,
    PRIMARY KEY (user_id, id)
);

-- Holdings（portfolio 快照）
CREATE TABLE holdings (
    user_id TEXT NOT NULL,
    snapshot_at TEXT NOT NULL,   -- 同一时刻的多行构成一个快照
    ticker TEXT NOT NULL,
    qty REAL NOT NULL,
    avg_cost REAL,
    notes TEXT,
    PRIMARY KEY (user_id, snapshot_at, ticker)
);

-- Reviews
CREATE TABLE reviews (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    decision_id TEXT,            -- 关联决策（可空：周期性整体复盘）
    summary TEXT NOT NULL,
    score INTEGER,               -- 自评 1-5
    lessons TEXT,
    corpus_inbox_refs_json TEXT, -- 写入 corpus/inbox 的候选清单（可空）
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

-- Watchlists
CREATE TABLE watchlists (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    tickers_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

-- Artifacts（agent 产出的中间产物 / panel 持久化）
CREATE TABLE artifacts (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,          -- chart | table | document | calculation | reasoning_dag | ...
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, session_id, id)
);
```

所有表均 user_id 在主键里，并建索引 `(user_id, created_at)` 等常用查询路径。

## Consequences

### Hybrid Storage 是系统形态

- Corpus：文件 + git（read-mostly）
- Vault：SQLite（read-write 高频）

跨域引用走**软引用**：vault 表的 `corpus_refs_json` 字段存路径字符串数组，渲染时 resolver 拿字符串去文件系统读。**不做双向 wikilink resolver、不做反向链表索引**——避免引入复杂度。

### Decision Artifact 不再是 markdown 文档

之前文件系统假设下，agent "输出 decision" = 写一个 `.md` 文件（带 frontmatter）。改 SQLite 后：

- agent 调用 `record_decision(...)` tool，结构化字段进 SQL 列
- 长 rationale 进 TEXT 字段，仍可用 markdown 撰写（前端 panel 渲染时按 markdown 解析）
- 前端 panel 直接 `SELECT` 出 form-style 视图，无需解析 frontmatter

### Schema Migration 用 sqlx

`migrations/` 目录下编号 SQL 文件，启动时自动检查并 apply。每个 schema 改动一个新 migration 文件，**不就地改老文件**。

### Cloud 切换 Postgres 几乎零改动

- 同 schema 直接拷过去（SQLite 与 Postgres 类型差异主要是 `INTEGER` vs `BIGINT` / `BOOLEAN`，可用 `feature flag` 在 sqlx 层透明）
- driver 从 `sqlx::Sqlite` 改 `sqlx::Postgres`
- 主键 `(user_id, id)` 形式天然适合 Postgres 的分区策略

### 不能 Obsidian 直接看 vault

接受这个代价。Vault 是动态运行时数据，由 agent 和 web frontend 维护；用户主入口是 web，不是 Obsidian。Corpus 仍可 Obsidian 编辑——两套数据两种工作流。

## Alternatives Considered

### 多文件 SQLite per-user（被否）
- 隔离虽强但 cloud 切 Postgres 时模型完全不同（多 schema vs 多列），返工成本高
- Cross-user 管理（admin / 统计）要 union 多文件，实现复杂

### 文件系统 + frontmatter（被否）
- 查询能力差（"找出所有 ticker = NVDA 且 status = confirmed 的决策"要遍历所有文件 grep）
- 并发写入冲突（agent 写 + user 编辑）
- schema 演化只能靠 lint，无强制约束

### Postgres 直接起步（被否）
- P1 本地部署增加依赖（要装 Postgres）
- SQLite 在单机长跑、单 writer 场景下性能完全够用
- Cloud 切换路径已经规划好（schema 兼容），不必现在就引入

## 验证标准

- 100 个并发 session × 每秒 10 条 message 的写入压力下，p95 延迟 < 10ms
- Migration 启动检查 < 50ms
- 单库切换到 Postgres 通过 driver 替换 + 不超过 5 个 SQL 语法 patch 完成
