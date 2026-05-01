# P1 Spec — Vault Data Schema

> Vault（SQLite，单库多 user_id）的完整 schema 定义、migration 顺序、写入路径、查询模式。

依赖：[ADR-0002](../decisions/0002-sqlite-vault-single-db.md)（vault 选型）、[ADR-0010](../decisions/0010-single-agent-coordinator-subagent.md)（subagent 模型）、[`interaction-model.md`](../interaction-model.md)（task lifecycle）。

## 1. 总览

P1 的 vault 包含以下表（按职责分组）：

```
身份与配置
  users
  team_charters
  llm_provider_configs

交互核心
  sessions
  tasks
  messages
  events                  # 事件流持久化
  artifacts               # 通用产出
  panels_layouts          # 用户的 panel 布局

任务执行
  task_assignments
  subagent_runs
  tool_call_runs
  reasoning_dag_traces

产出
  deliverables
  decisions               # decision_draft 类型 deliverable confirmed 后同步进来
  reviews                 # review 类型同上

资产与监控
  holdings                # portfolio 快照
  watchlists

诊断
  schema_migrations
  llm_usage_log
```

**约定**：
- 所有表第一列都是 `user_id TEXT NOT NULL`
- 时间字段统一用 ISO 8601 字符串（如 `2026-05-01T14:30:00.123Z`）
- ID 字段统一用 UUID v7（时间排序友好）字符串
- JSON 字段命名统一以 `_json` 结尾
- 软删除 / status 用枚举字符串（不用 INTEGER 标志位）
- 文本主语用 markdown 时字段名以 `_md` 结尾

## 2. 完整 schema

### 2.1 身份与配置

```sql
-- ──────────────────────────────────────────────
-- users: 用户档案
-- ──────────────────────────────────────────────
CREATE TABLE users (
    user_id TEXT PRIMARY KEY,
    display_name TEXT,
    timezone TEXT DEFAULT 'UTC',          -- IANA 时区
    locale TEXT DEFAULT 'zh-CN',
    created_at TEXT NOT NULL,
    last_active_at TEXT
);

-- ──────────────────────────────────────────────
-- team_charters: 团队工作章程（升级版的 mandate）
-- ──────────────────────────────────────────────
CREATE TABLE team_charters (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT 'default',
    
    -- charter 全部内容存 YAML / JSON（前端可视化编辑器读写）
    charter_json TEXT NOT NULL,
    -- 形态参考 interaction-model.md §6.1：
    --   {
    --     "style": ["long-term-fundamental", ...],
    --     "hard_limits": { "max_position_pct": 10, ... },
    --     "soft_preferences": { "preferred_sectors": [...], ... },
    --     "work_style": { "decision_verbosity": "detailed", ... }
    --   }
    
    active INTEGER NOT NULL DEFAULT 1,    -- 1 = 当前生效
    version INTEGER NOT NULL DEFAULT 1,   -- 每次编辑递增
    
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_charters_active ON team_charters(user_id, active);

-- ──────────────────────────────────────────────
-- llm_provider_configs: LLM provider 凭证 / 配置
-- ──────────────────────────────────────────────
CREATE TABLE llm_provider_configs (
    user_id TEXT NOT NULL,
    provider_name TEXT NOT NULL,          -- "codex_oauth" | "anthropic_api_key" | "openai_api_key"
    
    auth_kind TEXT NOT NULL,              -- "oauth" | "api_key"
    
    -- API key path：直接存（加密由 OS keyring 或文件权限保证）
    api_key_encrypted TEXT,
    
    -- OAuth path：access_token / refresh_token / expires_at
    oauth_access_token TEXT,
    oauth_refresh_token TEXT,
    oauth_expires_at TEXT,
    oauth_scope TEXT,
    
    -- 模型偏好
    default_model TEXT,                   -- e.g. "claude-opus-4-7" / "gpt-5"
    model_aliases_json TEXT,              -- {"reasoning": "claude-opus-4-7-thinking", "fast": "haiku-4-5"}
    
    -- 优先级与启用状态
    priority INTEGER NOT NULL DEFAULT 0,  -- 数字越大越优先
    enabled INTEGER NOT NULL DEFAULT 1,
    
    last_used_at TEXT,
    last_error TEXT,
    last_error_at TEXT,
    
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, provider_name)
);
```

### 2.2 交互核心

```sql
-- ──────────────────────────────────────────────
-- sessions: 用户的工作空间（chat thread 的容器）
-- ──────────────────────────────────────────────
CREATE TABLE sessions (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- UUID v7
    title TEXT,                           -- 默认 agent 自动总结
    
    -- 状态
    status TEXT NOT NULL DEFAULT 'active', -- "active" | "archived"
    
    -- 元数据
    pinned INTEGER NOT NULL DEFAULT 0,
    
    created_at TEXT NOT NULL,
    last_active_at TEXT NOT NULL,
    archived_at TEXT,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_sessions_active ON sessions(user_id, status, last_active_at DESC);

-- ──────────────────────────────────────────────
-- tasks: 核心交互单元
-- ──────────────────────────────────────────────
CREATE TABLE tasks (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- UUID v7
    session_id TEXT NOT NULL,
    
    -- 用户输入
    title TEXT NOT NULL,
    goal TEXT NOT NULL,
    constraints_json TEXT,                -- {scope, horizon, risk_budget, avoid, deadline}
    expected_deliverable TEXT NOT NULL,   -- "decision_draft" | "research_brief" | "review" | ...
    priority TEXT NOT NULL DEFAULT 'normal', -- "low" | "normal" | "high" | "urgent"
    
    -- Context chips (mention)
    context_refs_json TEXT,               -- ["@NVDA", "@portfolio:current", "@corpus:..."]
    
    -- 来源
    source TEXT NOT NULL DEFAULT 'user',  -- "user" | "quick" | "cron" | "agent_proposed"
    parent_task_id TEXT,                  -- fork 自其他任务时
    
    -- 状态机（详见 interaction-model.md §3.3）
    status TEXT NOT NULL DEFAULT 'draft',
    -- "draft" | "queued" | "in_progress" | "awaiting_user"
    -- | "delivered" | "confirmed" | "rejected" | "cancelled" | "failed"
    status_reason TEXT,                   -- 用户 reject 时的理由 / failure 时的 error 摘要
    
    -- 时间戳
    created_at TEXT NOT NULL,
    queued_at TEXT,
    started_at TEXT,
    delivered_at TEXT,
    closed_at TEXT,                       -- confirmed / rejected / cancelled / failed 任一进入时
    
    -- 资源使用
    tokens_used INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, session_id) REFERENCES sessions(user_id, id)
);

CREATE INDEX idx_tasks_session ON tasks(user_id, session_id, created_at DESC);
CREATE INDEX idx_tasks_status ON tasks(user_id, status, created_at DESC);
CREATE INDEX idx_tasks_priority ON tasks(user_id, status, priority, created_at);

-- ──────────────────────────────────────────────
-- messages: chat 消息（包括 task thread 内的追问）
-- ──────────────────────────────────────────────
CREATE TABLE messages (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,                 -- per-session 单调递增
    
    -- 关联 task（如果该消息属于某 task thread）
    task_id TEXT,
    
    role TEXT NOT NULL,                   -- "user" | "agent" | "system" | "tool"
    
    -- 内容（结构化，支持文本 + 内嵌 panel ref + ...）
    content_json TEXT NOT NULL,
    -- 形态：{ "type": "text", "text": "..." }
    --     | { "type": "panel_ref", "panel_id": "..." }
    --     | { "type": "tool_call_ref", "run_id": "..." }
    --     | { "type": "thinking", "text_md": "..." }
    
    created_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, session_id, seq)
);

CREATE INDEX idx_messages_task ON messages(user_id, task_id, seq) WHERE task_id IS NOT NULL;

-- ──────────────────────────────────────────────
-- events: 事件流持久化（全部经 EventBus 的事件都进这里）
-- ──────────────────────────────────────────────
CREATE TABLE events (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,                 -- per-session 单调递增（与 messages.seq 不共享）
    
    task_id TEXT,                         -- 大部分 event 关联到某 task
    
    kind TEXT NOT NULL,                   -- 见 api.md 中的 EventKind 枚举
    payload_json TEXT NOT NULL,
    
    -- 来源
    source TEXT,                          -- "main_agent" | "subagent:<run_id>" | "user" | "system" | "cron"
    
    ts TEXT NOT NULL,                     -- 事件发生时间（高精度）
    persisted_at TEXT NOT NULL,           -- 写入 vault 时间
    
    PRIMARY KEY (user_id, session_id, seq)
);

CREATE INDEX idx_events_task ON events(user_id, task_id, seq) WHERE task_id IS NOT NULL;
CREATE INDEX idx_events_kind ON events(user_id, session_id, kind, seq);

-- ──────────────────────────────────────────────
-- artifacts: 通用产出（panel 持久化、reasoning DAG 等）
-- ──────────────────────────────────────────────
CREATE TABLE artifacts (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- UUID v7
    session_id TEXT NOT NULL,
    task_id TEXT,
    
    kind TEXT NOT NULL,
    -- "panel:quote" | "panel:chart" | "panel:reasoning_dag"
    -- | "panel:corpus_brain_trace"
    -- | "diagram" | "calculation" | "table" | ...
    
    -- panel 状态（含展现配置 + 数据快照）
    payload_json TEXT NOT NULL,
    
    -- 关联
    parent_artifact_id TEXT,              -- 嵌套 / 引用其他 artifact
    
    -- 标记
    pinned INTEGER NOT NULL DEFAULT 0,    -- 用户钉住的 panel 不会被自动 GC
    
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_artifacts_session ON artifacts(user_id, session_id, created_at DESC);
CREATE INDEX idx_artifacts_task ON artifacts(user_id, task_id, created_at DESC) WHERE task_id IS NOT NULL;
CREATE INDEX idx_artifacts_kind ON artifacts(user_id, kind, updated_at DESC);

-- ──────────────────────────────────────────────
-- panels_layouts: 用户的 panel 布局偏好（per-session）
-- ──────────────────────────────────────────────
CREATE TABLE panels_layouts (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    
    layout_json TEXT NOT NULL,
    -- {"panels": [{"artifact_id": "...", "x": ..., "y": ..., "w": ..., "h": ..., "z": ...}, ...]}
    
    updated_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, session_id)
);
```

### 2.3 任务执行

```sql
-- ──────────────────────────────────────────────
-- task_assignments: 任务分配历史（P1 总是分配给 main agent，但 schema 留扩展位）
-- ──────────────────────────────────────────────
CREATE TABLE task_assignments (
    user_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    seq INTEGER NOT NULL,                 -- 一个 task 可以被重新指派多次
    
    assignee TEXT NOT NULL,               -- P1 = "main_agent"; 未来 = specialist agent name
    assigned_by TEXT NOT NULL,            -- "user" | "system" | "main_agent"
    reason TEXT,
    
    created_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, task_id, seq)
);

-- ──────────────────────────────────────────────
-- subagent_runs: subagent 执行记录
-- ──────────────────────────────────────────────
CREATE TABLE subagent_runs (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- run_id
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    parent_run_id TEXT,                   -- 主 agent 是 NULL；嵌套 subagent 是父 run_id（P1 不允许嵌套，但 schema 留位）
    
    spec_name TEXT NOT NULL,              -- "valuation_dcf" / "news_summary" / ...
    
    -- 输入
    scope_json TEXT NOT NULL,             -- SubagentScope 序列化
    input_json TEXT NOT NULL,             -- SubagentInput 序列化
    
    -- 输出（完成后填）
    output_json TEXT,                     -- SubagentOutput 序列化
    success INTEGER,                      -- 0/1
    error TEXT,
    
    -- 资源使用
    tokens_used INTEGER NOT NULL DEFAULT 0,
    turns INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    
    -- 时间戳
    spawned_at TEXT NOT NULL,
    completed_at TEXT,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_subagent_task ON subagent_runs(user_id, task_id, spawned_at DESC);

-- ──────────────────────────────────────────────
-- tool_call_runs: 工具调用记录
-- ──────────────────────────────────────────────
CREATE TABLE tool_call_runs (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- run_id
    session_id TEXT NOT NULL,
    task_id TEXT,
    
    -- 调用方
    invoker TEXT NOT NULL,                -- "main_agent" | "subagent:<run_id>"
    
    tool_name TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    
    -- 结果
    result_json TEXT,
    success INTEGER,                      -- 0/1
    error TEXT,
    
    -- 资源
    duration_ms INTEGER,
    
    started_at TEXT NOT NULL,
    completed_at TEXT,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_tool_runs_task ON tool_call_runs(user_id, task_id, started_at DESC);
CREATE INDEX idx_tool_runs_tool ON tool_call_runs(user_id, tool_name, started_at DESC);

-- ──────────────────────────────────────────────
-- reasoning_dag_traces: ReasoningDAG 节点 + 边（结构化存储）
-- ──────────────────────────────────────────────
CREATE TABLE reasoning_dag_traces (
    user_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    
    nodes_json TEXT NOT NULL,             -- [{ "id", "kind", "title", "details", "ts", "status", "subagent_run_id?" }]
    edges_json TEXT NOT NULL,             -- [{ "from", "to" }]
    
    updated_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, task_id)
);
```

### 2.4 产出

```sql
-- ──────────────────────────────────────────────
-- deliverables: 任务的产物
-- ──────────────────────────────────────────────
CREATE TABLE deliverables (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- UUID v7
    task_id TEXT NOT NULL,
    
    kind TEXT NOT NULL,
    -- "decision_draft" | "research_brief" | "review" | "comparison"
    -- | "morning_brief" | "free_form"
    
    payload_json TEXT NOT NULL,
    -- decision_draft 的形态见 §2.4.x decisions 表（payload 是同 schema 的 superset）
    
    -- 状态
    status TEXT NOT NULL,                 -- "draft" | "ready" | "confirmed" | "rejected"
    rejection_reason TEXT,
    
    created_at TEXT NOT NULL,
    ready_at TEXT,
    confirmed_at TEXT,
    rejected_at TEXT,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_deliverables_task ON deliverables(user_id, task_id);

-- ──────────────────────────────────────────────
-- decisions: confirmed 的 decision_draft 同步进来
-- ──────────────────────────────────────────────
CREATE TABLE decisions (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    deliverable_id TEXT NOT NULL,         -- 关联到产出它的 deliverable
    task_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    
    -- 决策核心
    ticker TEXT NOT NULL,
    direction TEXT NOT NULL,              -- "long" | "short" | "close" | "adjust"
    size_shares REAL,
    size_pct REAL,
    stop_loss REAL,
    target REAL,
    horizon_days INTEGER,
    
    -- 论证
    rationale_md TEXT NOT NULL,
    corpus_refs_json TEXT,                -- ["wikis/principles/margin-of-safety.md", ...]
    
    -- Mandate check 快照（confirm 时点的检查结果）
    mandate_check_json TEXT,
    -- [{kind, severity, message}, ...]
    
    -- 复盘 schedule
    review_schedule_json TEXT,            -- ["2026-06-01", "2026-08-01"]
    review_done_dates_json TEXT,          -- 已经 review 过的日期
    
    -- 状态
    status TEXT NOT NULL DEFAULT 'open',  -- "open" | "closed" | "superseded"
    superseded_by TEXT,                   -- 如果被另一个 decision 覆盖
    
    -- 时间戳
    confirmed_at TEXT NOT NULL,
    closed_at TEXT,
    
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, deliverable_id) REFERENCES deliverables(user_id, id)
);

CREATE INDEX idx_decisions_ticker ON decisions(user_id, ticker, confirmed_at DESC);
CREATE INDEX idx_decisions_status ON decisions(user_id, status, confirmed_at DESC);
CREATE INDEX idx_decisions_review ON decisions(user_id, status) WHERE status = 'open';

-- ──────────────────────────────────────────────
-- reviews: 复盘记录
-- ──────────────────────────────────────────────
CREATE TABLE reviews (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    deliverable_id TEXT,                  -- 来自某个 review 类型的 deliverable
    
    decision_id TEXT,                     -- 关联的决策（可空：周期性整体复盘）
    period_start TEXT,                    -- 周期复盘的时间窗
    period_end TEXT,
    
    summary_md TEXT NOT NULL,
    self_score INTEGER,                   -- 1-5 自评
    agent_score INTEGER,                  -- 1-5 agent 评估
    lessons_md TEXT,
    
    -- 是否产生 corpus 候选（P1 不实现 promotion，但记录意图）
    corpus_inbox_candidates_json TEXT,
    
    created_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_reviews_decision ON reviews(user_id, decision_id) WHERE decision_id IS NOT NULL;
```

### 2.5 资产与监控

```sql
-- ──────────────────────────────────────────────
-- holdings: portfolio 快照（详见 ADR-0009）
-- ──────────────────────────────────────────────
CREATE TABLE holdings (
    user_id TEXT NOT NULL,
    snapshot_at TEXT NOT NULL,
    ticker TEXT NOT NULL,
    
    qty REAL NOT NULL,
    avg_cost REAL,
    
    -- 元数据
    account TEXT,                         -- "main" / 用户自定义账户名（P1 默认 "main"）
    notes TEXT,
    source TEXT NOT NULL,                 -- "manual" | "csv_import" | "broker_api"（P1 = manual / csv_import）
    
    PRIMARY KEY (user_id, snapshot_at, ticker, account)
);

CREATE INDEX idx_holdings_user_recent ON holdings(user_id, snapshot_at DESC);

-- ──────────────────────────────────────────────
-- watchlists: 自选股
-- ──────────────────────────────────────────────
CREATE TABLE watchlists (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    tickers_json TEXT NOT NULL,           -- ["NVDA", "GOOGL", ...]
    
    -- 排序与显示
    sort_order INTEGER NOT NULL DEFAULT 0,
    
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, id)
);
```

### 2.6 诊断

```sql
-- ──────────────────────────────────────────────
-- schema_migrations: sqlx 标准迁移记录
-- ──────────────────────────────────────────────
CREATE TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);

-- ──────────────────────────────────────────────
-- llm_usage_log: LLM 调用统计（cost / token / latency）
-- ──────────────────────────────────────────────
CREATE TABLE llm_usage_log (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,                     -- UUID v7
    
    provider_name TEXT NOT NULL,
    model TEXT NOT NULL,
    
    -- 调用方
    invoker TEXT NOT NULL,                -- "main_agent" | "subagent:<run_id>" | "system:<...>"
    session_id TEXT,
    task_id TEXT,
    
    -- 资源使用
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL,
    
    -- 状态
    success INTEGER NOT NULL,
    error TEXT,
    
    started_at TEXT NOT NULL,
    
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_usage_provider ON llm_usage_log(user_id, provider_name, started_at DESC);
CREATE INDEX idx_usage_task ON llm_usage_log(user_id, task_id, started_at DESC) WHERE task_id IS NOT NULL;
```

## 3. Migration 顺序

P1 启动只需要一个初始 migration（`0001_initial.sql`），按上面的 schema 全部建表。

后续演化遵循以下原则：

1. **每个 schema 改动一个新 migration 文件**，编号递增（`0002_<name>.sql`）
2. **不就地改老 migration 文件**——已经在生产数据库上 apply 过的不能改
3. **加字段优先于改字段**——加 `ALTER TABLE ADD COLUMN` 是 backward-compatible 的；改字段类型 / 改约束需要 `CREATE TABLE_v2 + INSERT INTO ... SELECT + DROP_v1`
4. **下线字段前先 deprecate**——保留 ≥ 2 个版本周期再删
5. **加索引随时**：`CREATE INDEX IF NOT EXISTS` 是安全的

启动时通过 sqlx 自动检查 + apply 未执行的 migration。

## 4. 写入路径与权限边界

| 表 | 谁写 | 触发 |
|--|--|--|
| `users` | gateway 启动时 | 首次启动 ensure user_id |
| `team_charters` | 用户（前端 form） | 编辑 charter |
| `llm_provider_configs` | 用户（前端 settings） | 添加 / 切换 provider |
| `sessions` | gateway | 用户开新 session |
| `tasks` | gateway | 用户提交 task / cron 创建 / agent 提议 |
| `messages` | gateway | 每条 chat message |
| `events` | gateway (EventBus persistence consumer) | 每条 event 异步 batch 写入 |
| `artifacts` | gateway / main agent | panel 创建 / 更新 |
| `panels_layouts` | gateway | 用户拖动调整布局 |
| `task_assignments` | gateway | task 创建 / 重指派 |
| `subagent_runs` | main agent | spawn / complete |
| `tool_call_runs` | main agent / subagent runner | tool dispatch / complete |
| `reasoning_dag_traces` | main agent | reasoning 节点 / 边产生 |
| `deliverables` | main agent | 撰写 / 用户 confirm/reject |
| `decisions` | gateway（confirm 触发的派生写入） | deliverable confirmed 时同步 |
| `reviews` | gateway（同上） | review deliverable confirmed |
| `holdings` | 用户（前端 form / CSV import） | 用户更新 portfolio |
| `watchlists` | 用户（前端 form） | 编辑 watchlist |
| `llm_usage_log` | gateway | 每次 LLM 调用结束 |

**Subagent 不能写任何表**——它的输出是返回给 main agent，main agent 决定写不写。这是 ADR-0010 的核心边界。

## 5. 高频查询模式

### 5.1 Session 列表（首页）

```sql
SELECT id, title, status, last_active_at,
       (SELECT COUNT(*) FROM tasks WHERE user_id = ? AND session_id = sessions.id 
        AND status IN ('queued', 'in_progress', 'awaiting_user')) AS open_tasks
FROM sessions
WHERE user_id = ? AND status = 'active'
ORDER BY pinned DESC, last_active_at DESC
LIMIT 50;
```

### 5.2 TaskBoard（按 status 分组的看板）

```sql
SELECT id, title, goal, expected_deliverable, priority, status,
       created_at, started_at, delivered_at
FROM tasks
WHERE user_id = ? AND status NOT IN ('confirmed', 'rejected', 'cancelled')
ORDER BY 
  CASE priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'normal' THEN 2 ELSE 3 END,
  created_at DESC;
```

### 5.3 某 Task 的完整 trace

```sql
-- task 元数据
SELECT * FROM tasks WHERE user_id = ? AND id = ?;

-- 该 task 的所有 messages
SELECT * FROM messages WHERE user_id = ? AND task_id = ? ORDER BY seq;

-- 该 task 的所有 events（用于回放）
SELECT * FROM events WHERE user_id = ? AND task_id = ? ORDER BY seq;

-- 该 task 的所有 tool calls
SELECT * FROM tool_call_runs WHERE user_id = ? AND task_id = ? ORDER BY started_at;

-- 该 task 的所有 subagent runs
SELECT * FROM subagent_runs WHERE user_id = ? AND task_id = ? ORDER BY spawned_at;

-- 该 task 的 reasoning DAG
SELECT * FROM reasoning_dag_traces WHERE user_id = ? AND task_id = ?;

-- 该 task 的所有 artifact (panel)
SELECT * FROM artifacts WHERE user_id = ? AND task_id = ? ORDER BY created_at;

-- 该 task 的 deliverables
SELECT * FROM deliverables WHERE user_id = ? AND task_id = ? ORDER BY created_at;
```

### 5.4 待复盘的 decision

```sql
SELECT * FROM decisions
WHERE user_id = ? 
  AND status = 'open'
  AND json_each.value IN (
      SELECT value FROM json_each(decisions.review_schedule_json)
      WHERE value <= date('now')
        AND value NOT IN (SELECT value FROM json_each(coalesce(decisions.review_done_dates_json, '[]')))
  );
```

P1 简化版：startup 时跑一次扫描即可，不必上面这么花哨；可以先在应用层做 review schedule 比较。

### 5.5 LLM usage 日报

```sql
SELECT provider_name, model,
       SUM(input_tokens) AS in_tok, SUM(output_tokens) AS out_tok,
       SUM(duration_ms) AS total_ms,
       COUNT(*) AS calls
FROM llm_usage_log
WHERE user_id = ? AND started_at >= date('now', '-1 day')
GROUP BY provider_name, model
ORDER BY in_tok + out_tok DESC;
```

## 6. SQLite 配置

启动时 PRAGMA：

```sql
PRAGMA journal_mode = WAL;          -- 并发读优于默认 rollback journal
PRAGMA synchronous = NORMAL;        -- WAL 下足够安全
PRAGMA foreign_keys = ON;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;       -- 256 MB
PRAGMA busy_timeout = 5000;         -- 5s
```

## 7. 容量与性能预估

| 实体 | 单条大小 | 假设增长率 | 1 年累计 | 备注 |
|--|--|--|--|--|
| sessions | ~200 B | 1/天 | ~73 KB | |
| tasks | ~1 KB | 5/天 | ~1.8 MB | |
| messages | ~500 B | 50/天 | ~9 MB | |
| events | ~300 B | 200/天 | ~21 MB | 主要存储压力 |
| artifacts | ~5 KB（panel state）| 30/天 | ~55 MB | |
| reasoning_dag_traces | ~10 KB | 5/天 | ~18 MB | |
| llm_usage_log | ~200 B | 100/天 | ~7 MB | |
| **合计** | — | — | **~110 MB** | 单用户单年 |

SQLite 完全胜任。WAL 模式下并发读约 ~10k QPS，写约 ~1k QPS，远超 P1 单用户场景。

Cloud 切换 Postgres 时同样的数据量是分钟级别的迁移。

## 8. 数据保留策略（P1 简化）

P1 不主动 GC 任何数据——保留全量历史。即使是 `events` 表的高频写入，1 年累积 21 MB 也不构成问题。

未来需要 GC 时（P2+）：
- `events` 中老的低价值 event（如 tick price update）可以定期 archive 到压缩文件
- 已 confirmed/rejected 任务的 ephemeral artifact 可以清理（保留 deliverable 即可）

## 9. 与 corpus 的关系

Vault **不存 corpus 内容**——corpus 是文件系统的静态资源（详见 [ADR-0003](../decisions/0003-corpus-as-static-resource.md)）。

跨域引用通过软引用：vault 中各表的 `corpus_refs_json` / `corpus_inbox_candidates_json` 字段存路径字符串数组（如 `["wikis/principles/margin-of-safety.md"]`），渲染时 resolver 拿字符串去文件系统读。

不在 vault 建 corpus 反向索引——这种查询足够稀有，临时全表扫描可接受。

## 10. 实施 checklist

- [ ] `0001_initial.sql` 包含上面所有 CREATE TABLE / CREATE INDEX
- [ ] 启动时 sqlx 自动 apply migration
- [ ] 每个表的 Rust struct 定义（`crate::vault::models::*`）
- [ ] Repository 层（`crate::vault::repos::*`）封装常用查询
- [ ] 单元测试：每个高频查询有对应测试
- [ ] e2e 测试：完整 task lifecycle 的状态转换 + 数据完整性验证
- [ ] backup tool：`leek vault backup` 命令导出整个 DB（cp + WAL checkpoint）
