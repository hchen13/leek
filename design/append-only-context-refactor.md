# leek append-only context 改造方案（codex 范式对齐）

> **执行规范 (executable spec)** — 可被 `/goal` 命令直接执行。
> 由 append-only-context 改造 workflow(5 路深度 code review + codex 榜样深挖)综合生成,所有 file:line 已对照源码核实。
> 榜样 = codex(我们用同一个 OpenAI Responses API,`ResponseItem` 零翻译对应)。
> 执行入口见 §11 分步 workflow,验收见 §12 checklist。
> 已经过多轮 owner 决策细化:术语(turn/iteration)、工具输出控量(主 CC + codex 兜底、确定性需求在代码层解决而非靠 agent 摆姿势)、compaction 阈值动态化(max_context×90%)与双触发(pre-turn + mid-turn)、工具 N-turn 降级、老数据可删。

## 1. 背景与问题

leek 当前用 OpenAI Responses API（codex backend, `chatgpt.com/backend-api/codex/responses`）跑主 agent loop，但上下文管理偏离 codex 范式，已确诊四个根因级问题（均已对照源码核实）：

**问题 1：tool call/result 不持久化。** tool 痕迹只存在单次 reply 的局部变量 `additional_inputs: Vec<serde_json::Value>`（`agent/mod.rs:182`），每个 call 执行后在 `agent/mod.rs:773-783` push 两条原始 ResponseItem（`{type:function_call}` + `{type:function_call_output}`）。这个 Vec 随 `run_chat_reply` 返回即丢。`vault.messages` 在 `agent/mod.rs:829-838` 只写 `{type:text, text:final_text}`（role=`agent`）。下一个 turn 调 `run_chat_reply` 从 `agent/mod.rs:111` 重读历史，`agent/mod.rs:130-145` 的 filter 只认 `user`/`agent` 两个 role 且只取 `content.text`，tool 痕迹全部丢失。

**问题 2：跨 turn 靠每 iteration 重算的 SESSION STATE 摘要回灌，破坏 prompt cache。** `build_session_state_inputs`（`agent/mod.rs:917-984`）取最近 12 条 `tool_call_runs`、plan、web_search 事件，拼成一条 role=`user` 的 "SESSION STATE (read-only runtime context)" 文本块，每条 tool 证据经 `format_tool_run_for_state`（`agent/mod.rs:986-999`）截断 800 字。它在 `agent/mod.rs:177` 算一次但在 `agent/mod.rs:253` 每 iteration `.clone()` 重放，加上 plan reminder（`agent/mod.rs:255-266`）、web guard（`agent/mod.rs:267-271`）也每 iteration 重建并插在 input 末尾 → cache 前缀逐 iteration 漂移，大面积 miss。

**问题 3：发给模型的 tool 输出是改写压缩版。** `agent/mod.rs:719` 用 `model_output = compact_tool_output_for_model(name, output)`（`agent/mod.rs:1216-1233`，逐行白名单改写、丢 URL、截表格），`agent/mod.rs:782` 的 `function_call_output.output` 用的是 `model_output` 而非原始 `output_str`。模型看到的不是它真正拿到的数据，跨 turn 无法靠队列重建真实证据。

**问题 4：auto-compaction 阈值失效 + 无「模型 context 上限」概念。** `api/messages.rs:23` 硬编码 `AUTO_COMPACT_THRESHOLD_DEFAULT = 380_000`，注释（`:20-21`）自述「95% of the 400K context window」——但这个 400K 是**错误假设**：codex backend 的 `gpt-5.5` 实际 max context 是 **272K**。`380K > 272K`，意味着 input 涨到 272K 时模型先报 context 超限，**永远到不了 380K → auto-compaction 形同虚设、从未真正触发**。更深层：代码里 grep 不到任何「模型 → context 上限」映射（零定义），阈值是写死的绝对数而非 `context上限 × 比例`。这正解释了「leek 不会 auto-compact 而 codex 会」的体感差异。修法见 §8.1。

**命名 bug：** `agent/mod.rs:185` `let mut turn = 0usize` + `agent/mod.rs:820` `turn += 1`，这个 `turn` 语义其实是 iteration（一个 user→assistant 应答内部的 tool-call 循环计数）。`plan_last_update_turn`（`:183`）、`plan_last_reminder_turn`（`:184`）、`MAX_TOOL_TURNS`（`:37`）、`PLAN_REMINDER_INTERVAL_TURNS`（`:43`）、`active_plan_reminder_input`/`plan_reminder_tone` 的 `turn` 参数（`:1676`/`:1699`）同理。真正的 turn（一次 `run_chat_reply` 调用）当前无显式实体。

**目标：** 改成 codex / Claude Code 那样真正的 append-only 队列——完整保存 tool 痕迹（function_call / function_call_output）、按原样按序回放、撑到 token 接近上限才 compaction。

---

## 2. 榜样选择：照抄 codex

**选 codex（不是 Claude Code）的硬理由：我们用同一个 Responses API。** codex-rs 的 `ResponseItem` 枚举（`function_call` / `function_call_output` / `message` / `reasoning` / `compaction`）与我们 `additional_inputs` 里塞的 raw item 一一对应，`build_request_body`（`llm/openai_responses.rs:69-75`）直接 `extend` 进 `input` 数组，零翻译成本。Claude Code 走 Anthropic Messages API（`tool_use`/`tool_result` block 嵌在 message 里、结构不同），照抄它反而要做一层 schema 转换。

**直接照抄的 4 条 codex 机制：**

1. **ResponseItem 作为统一持久化原子。** `function_call`/`function_call_output` 是一等公民，与 user/assistant message 同级，按**原样**进 history，不摘要、不改写、不局部变量。
2. **每个 iteration 把整个 history Vec 原样回放。** codex `get_formatted_input(&self) -> Vec<ResponseItem> { self.input.clone() }` 仅 clone 整个 input 数组，没有任何 per-iteration 重算/截断。
3. **初始 context 一次性 append 且之后不变**（UserInstructions → EnvironmentContext），作为稳定前缀。
4. **append-only 保 cache：** 任何 runtime 变化都在尾部追加，绝不改前缀 → exact-prefix 命中。

**我们与 codex 的差异适配（rollout JSONL → sqlite vault）：**

| codex | leek |
|---|---|
| 一 session 一个 rollout JSONL 文件，每行一个 timestamped `RolloutLine{RolloutItem}` | 多 session 共一个 `vault.messages` 表，`(user_id, session_id, seq)` 单调序做分区+排序 |
| 内存态 `ConversationHistory: Vec<ResponseItem>` | 每个 turn 从 DB 按 seq 读回，单 turn 内增量 append |
| `RolloutRecorder` 异步后台写 + SQLite 索引做 thread 发现 | 我们本就在 SQLite，session 列表直接查 DB，少一层 |
| 服务端 `/responses/compact` 返回 `type=compaction` encrypted item | **我们暂不用 `/responses/compact`**（见 §8），继续用本地 LLM 蒸馏成 `compaction_summary`，但触发与边界对齐 codex 双触发语义 |
| `reconstruct_history_from_rollout` resume 从 transcript 重建 | `run_chat_reply` 从 `vault.messages` tail 重建 input |

> **决策（自主拍板）：** 复用 `messages` 表承载 tool 痕迹（新增 role），**不新建 `turn_items` 表**。理由：`messages` 已有 `(user_id,session_id,seq)` append-only 单调序、已被 hard_delete/compaction/前端复用；tool 痕迹与 user/agent 文本共享同一条 seq 时间线，天然保证「call 在 output 前」「assistant 文本与 tool 调用顺序一致」，无需跨表 merge-sort。新表会引入第二条 seq 序列与 messages 交织难题。代价是 role 语义膨胀 + 需在 UI/compaction 路径审计 role 分支——本方案已覆盖这些审计点（§6/§7/§8）。

---

## 3. 目标状态：append-only 队列完整数据流

```
┌─ 一个 TURN = run_chat_reply 一次调用（user message → assistant message）─┐
│                                                                          │
│  [POST /messages]                                                        │
│    └─ vault_messages::insert(role="user", {type:text,text})  ──► seq=N   │
│                                                                          │
│  run_chat_reply 开始:                                                    │
│    1. vault_messages::list 全量读 ──► all_history                        │
│    2. rposition(role=="compaction_summary") 切 tail (不变)              │
│    3. 回放重建: tail 每行 row_to_input_item() 按 seq 顺序展开            │
│         user            ──► {role:user, content}                         │
│         agent           ──► {role:assistant, content}                    │
│         assistant_tool_calls ──► [展开 function_call 数组]               │
│         tool_result     ──► {type:function_call_output}                  │
│         compaction_summary ──► (走 runtime_context_messages 前置注入)    │
│       得到 replay_inputs  ◄─── 稳定前缀，整个 turn 内只在尾部 append      │
│                                                                          │
│    ┌─ ITERATION 循环 (内部 tool-call 轮次, 旧名 turn) ─────────────┐     │
│    │  request_inputs = replay_inputs.clone()                      │     │
│    │    (+ 末尾瞬时 hint: 降频 plan reminder / budget note)        │     │
│    │  provider.chat(req)  ──► stream                              │     │
│    │  收 FunctionCall → pending_calls                             │     │
│    │  pending_calls 空 ──► final_text=turn_text, break            │     │
│    │  否则 dispatch 每个 call:                                    │     │
│    │    vault_tool_runs::start/finish (诊断/UI源, 不变)           │     │
│    │    ┌─ 落库 append-only (本方案核心) ─────────────────┐       │     │
│    │    │ (若 turn_text 非空先) insert(role=agent, text)  │       │     │
│    │    │ insert(role=assistant_tool_calls, [func_calls]) │seq+1  │     │
│    │    │ insert(role=tool_result, {func_call_output,     │seq+2  │     │
│    │    │        output: 原始 output_str})  ◄── 不是 model_output │     │
│    │    └────────────────────────────────────────────────┘       │     │
│    │  把新落库的 item append 到 replay_inputs 尾部                │     │
│    │  iteration += 1                                              │     │
│    └──────────────────────────────────────────────────────────────┘     │
│                                                                          │
│  循环退出: insert(role="agent", {type:text, final_text})  ──► seq=M      │
│  publish agent_message_end                                               │
└──────────────────────────────────────────────────────────────────────────┘

下一个 TURN: vault_messages::list 读回的 tail 已含 assistant_tool_calls / tool_result 行
            ──► 模型原样看到上个 turn 调过什么工具、拿到什么原始数据
            ──► prompt cache: 前缀逐 byte 稳定 → cache_read_tokens 显著上升
```

**关键不变量（codex 范式）：**
- function_call 必须在其 function_call_output 之前（seq 保证）。
- assistant message 必须在其 function_call 之前（落库顺序保证）。
- replay_inputs 前缀逐 iteration byte-identical，只在尾部 append → cache 命中。

---

## 4. 术语统一

**定义（全程统一）：**
- **turn** = 一次「user message → assistant message」的完整应答 = **一次 agent loop** = 一次 `run_chat_reply` 调用。**无需计数器**，概念上对应整个函数。
- **iteration** = 一个 turn(agent loop)内部的若干次循环迭代（每次 `provider.chat` + 一轮 tool dispatch；对应 codex `run_turn` loop / Anthropic 一轮 tool_use/tool_result）。代码实现里它**就是**那个 `loop` 的迭代次数,所以叫 iteration。

**rename 清单（机械改名，纯语义不改逻辑，精确到 file:line）：**

| 当前 | 改为 | 位置 |
|---|---|---|
| `const MAX_TOOL_TURNS` | `MAX_TOOL_ITERATIONS` | `agent/mod.rs:37` 定义；引用 `:202`、`:223`（format string `{MAX_TOOL_TURNS} 轮`） |
| `const PLAN_REMINDER_INTERVAL_TURNS` | `PLAN_REMINDER_INTERVAL_ITERATIONS` | `agent/mod.rs:43` 定义；引用 `:1707` |
| `let mut turn = 0usize` | `let mut iteration = 0usize` | `agent/mod.rs:185` 声明 |
| `turn += 1` | `iteration += 1` | `agent/mod.rs:820` |
| `turn` 引用 | `iteration` | `agent/mod.rs:202`、`:259`、`:602`、`:613`、`:710`（`plan_last_update_turn = turn + 1`） |
| `plan_last_update_turn` | `plan_last_update_iteration` | `agent/mod.rs:183` 声明；`:260`、`:710`、`:1677`、`:1700`、`:1703`、`:1706` |
| `plan_last_reminder_turn` | `plan_last_reminder_iteration` | `agent/mod.rs:184` 声明；`:261`、`:711`、`:1678`、`:1691`、`:1701`、`:1710` |
| `active_plan_reminder_input(... turn ...)` 形参 | `iteration` | `agent/mod.rs:1676` 签名 + body |
| `plan_reminder_tone(turn, ...)` 形参 | `iteration` | `agent/mod.rs:1698-1718` 签名 + body |
| `publish_agent_trace_note(... turn ...)`、`flush_agent_narration(... turn ...)`、`format_tool_batch_trace` 间接 | `iteration` | `agent/mod.rs:602`、`:613` 调用处 + 各自签名 |

**事件 payload 的 `turn` key（`agent/mod.rs` narration/trace_note payload）：** 保留 key 名 `"turn"` 不改（前端可能依赖），仅把绑定的值从 `turn` 变量改为 `iteration` 变量。**这是为了不破前端 SSE 契约。** 若后续要彻底改名需同步前端。

**stop_reason `max_tool_turns_finalized`（`agent/mod.rs:203`）：** 保留字面量不改（它是对外契约字符串，前端可能匹配）。

**用户可见中文文案 `{MAX_TOOL_TURNS} 轮`（`agent/mod.rs:223`）：** rename 常量后文案变 `{MAX_TOOL_ITERATIONS} 轮`，「轮」指 iteration，语义仍正确，保持中文不变。

**验证：** `cargo build` 通过；`grep -n '\bturn\b' crates/gateway/src/agent/mod.rs` 仅剩注释中描述真 turn 概念处、`'turns` label（可一并改 `'iterations`）、event payload key `"turn"`、stop_reason 字面量。

---

## 5. 数据模型变更

### 5.1 新 migration `0009_conversation_queue.sql`

**决策：不改 `messages` 表结构**（已有 `role TEXT` / `content_json TEXT` 足够），仅加一个覆盖「回放可见行」的部分索引，加速 replay 过滤扫描。零迁移风险、可空跑、无 DROP/RENAME。

```sql
-- 0009_conversation_queue.sql
-- messages 升级为 append-only conversation queue:新增两个 role 承载
-- ResponseItem 原样痕迹,与 user/agent/compaction_summary 共享同一条 seq 轴。
--
-- role 枚举(应用层约束,沿用现有 compaction_summary 的无 CHECK 风格):
--   'user'                  -- 既有: 用户文本   {type:text,text}
--   'agent'                 -- 既有: assistant 最终文本 {type:text,text}
--   'compaction_summary'    -- 既有: compaction 边界 {type:text,text}
--   'assistant_tool_calls'  -- 新增: 一个 iteration 触发的一批 function_call
--   'tool_result'           -- 新增: 对应的 function_call_output (每 call_id 一行)
--
-- content_json 形态见 §5.2。历史行 role 仍是 user/agent/compaction_summary,
-- 本来就是合法回放项,无需回填。

CREATE INDEX IF NOT EXISTS idx_messages_replay
    ON messages(user_id, session_id, seq)
    WHERE role IN ('user', 'agent', 'compaction_summary',
                   'assistant_tool_calls', 'tool_result');
```

**验证点：** `cargo sqlx migrate run` 后 `SELECT name FROM sqlite_master WHERE type='index' AND name='idx_messages_replay'` 返回一行；既有 `vault.db` 跑完无报错、`SELECT count(*) FROM messages` 行数不变。

> **为何不加 `turn_id`/`iteration`/`call_id` 列：** 本期回放只需按 seq 排序 + role 分派，不需要按 turn/call_id 索引查询。`call_id` 已存在 `content_json` 内（见 §5.2）。保持最小改动。若未来要按 turn 维度分析再加列（向后兼容加列即可）。

### 5.2 content_json 存储结构（每种新 role 一种 schema）

回放时要把 DB 行原样还原成可直接 `extend` 进 Responses API `input` 数组的 raw item，因此 content_json 必须就是 raw item 或其无损封装。

```jsonc
// role = 'user'           (不变)
{"type":"text","text":"..."}

// role = 'agent'          (不变)
{"type":"text","text":"..."}

// role = 'compaction_summary'  (不变)
{"type":"text","text":"..."}

// role = 'assistant_tool_calls'  (新增) — 一个 iteration 内可有多个并发 call
{"type":"tool_calls","items":[
   {"type":"function_call","call_id":"...","name":"...","arguments":"<raw json string>"},
   ...
]}
// arguments 存模型给的原始字符串(call.arguments),不重新序列化。

// role = 'tool_result'  (新增) — 每个 call_id 一行
{"type":"function_call_output","call_id":"...","output":"<原始全量 output_str>"}
// 关键: 存 agent/mod.rs:722 的原始 output_str,不是 model_output(compact 改写版)。
```

### 5.3 回放反序列化辅助（`vault/messages.rs` 新增）

```rust
/// 把一行 message 还原成 Responses API input 项。
/// 返回 Vec 因为 assistant_tool_calls 一行展开成多个 function_call item。
/// compaction_summary 返回 None(走 runtime_context_messages 前置注入)。
pub fn row_to_input_items(role: &str, content_json: &str) -> Vec<serde_json::Value> {
    let v: serde_json::Value = match serde_json::from_str(content_json) {
        Ok(v) => v, Err(_) => return Vec::new(),
    };
    match role {
        "user" => vec![json!({"role":"user","content": v.get("text").and_then(|t|t.as_str()).unwrap_or("")})],
        "agent" => vec![json!({"role":"assistant","content": v.get("text").and_then(|t|t.as_str()).unwrap_or("")})],
        "assistant_tool_calls" => v.get("items").and_then(|i|i.as_array())
            .map(|arr| arr.to_vec()).unwrap_or_default(),
        "tool_result" => vec![v],            // {type:function_call_output,...} 原样
        _ => Vec::new(),                     // compaction_summary 等
    }
}

/// UI 列表只读:过滤掉 tool-trace role。
pub async fn list_for_ui(pool, user_id, session_id, since_seq, limit) -> Result<Vec<MessageRow>>;
// SQL: WHERE role IN ('user','agent','compaction_summary')

/// LLM replay 全量读(沿用现有 list,无需改)。
```

**验证点：** 单测 round-trip——构造 `function_call`+`function_call_output` 写入、`list` 读回、`row_to_input_items` 还原，断言 `call_id`/`arguments`/`output` 与写入 byte-equal。

---

## 6. agent loop 重写

### 6.1 新增落库辅助（`agent/mod.rs`）

```rust
/// 落一行 assistant_tool_calls(一个 iteration 的一批 function_call)。
async fn persist_tool_calls(pool, user_id, session_id, calls: &[PendingCall]) -> Result<i64> {
    let items: Vec<_> = calls.iter().map(|c| json!({
        "type":"function_call", "call_id": c.call_id, "name": c.name, "arguments": c.arguments
    })).collect();
    vault_messages::insert(pool, user_id, session_id, "assistant_tool_calls",
        &json!({"type":"tool_calls","items": items}), None).await
}

/// 落一行 tool_result(原始 output,不是 model_output)。
async fn persist_tool_result(pool, user_id, session_id, call_id: &str, output_str: &str) -> Result<i64> {
    vault_messages::insert(pool, user_id, session_id, "tool_result",
        &json!({"type":"function_call_output","call_id": call_id, "output": output_str}), None).await
}
```

### 6.2 dispatch 循环改造（替换 `agent/mod.rs:769-783`）

```rust
// 现状 :773-783 两处 additional_inputs.push 全部删除,替换为:

// (A) 进入 dispatch 前(在 :640 for 循环之前),若本 iteration assistant 已产出
//     文本且即将触发 tool,先落 agent 文本行,保证文本在 function_call 之前
//     (codex 要求 assistant message 在其 function_call 前)。
if !turn_text.trim().is_empty() {
    let seq = vault_messages::insert(pool, user_id, session_id, "agent",
        &json!({"type":"text","text": turn_text.clone()}), None).await?;
    replay_inputs.push(json!({"role":"assistant","content": turn_text.clone()}));
    // 注意:此 iteration 的文本已落库,turn 末不再重复落(见 6.4)。
    mid_iteration_text_persisted = true;
}

// (B) 一次性落一行 assistant_tool_calls,并 append 到 replay 尾部。
let calls_seq = persist_tool_calls(pool, user_id, session_id, &pending_calls).await?;
for c in &pending_calls {
    replay_inputs.push(json!({"type":"function_call","call_id":c.call_id,"name":c.name,"arguments":c.arguments}));
}

// (C) 每个 call dispatch+finish 后(在 :733 vault_tool_runs::finish 之后),落 tool_result
//     用原始 output_str(:683 那条),并 append。
persist_tool_result(pool, user_id, session_id, &call.call_id, &output_str).await?;
replay_inputs.push(json!({"type":"function_call_output","call_id":call.call_id,"output":output_str}));
```

**关键纠正：** `agent/mod.rs:719` 的 `compact_tool_output_for_model` 不再用于落库/回放（见 §7.4）。`tool_result.output` 和 `replay_inputs` 用原始 `output_str`。

> **原子性（risk 缓解）：** 同一 iteration 的 `assistant_tool_calls` 行 + 全部 `tool_result` 行应在一个逻辑单元内落完。本期 dispatch 串行（`agent/mod.rs:640` `for call in pending_calls` 顺序），崩溃可能留下「有 assistant_tool_calls 但部分 tool_result 缺失」。**缓解：回放时按 call_id 配对校验，丢弃无配对 output 的 function_call**（见 §6.3 步骤 4），避免 codex backend 报 orphan call_id。

### 6.3 input 回放重建（替换 `agent/mod.rs:111-152` + `:177` + `:253-254`）

```rust
let all_history = vault_messages::list(&pool, &user_id, &session_id, None, 1000).await?;

// 1) 切 tail(不变): 找最后 compaction_summary,其 text 进 handoff_summaries。
let mut handoff_summaries = Vec::new();
let tail_start = all_history.iter().rposition(|r| r.role == "compaction_summary")
    .map(|i| { /* 收 summary text,同 :121-125 */ i + 1 }).unwrap_or(0);

// 2) 回放: tail 每行展开成 input items,按 seq 顺序。
let mut replay_inputs: Vec<Value> = Vec::new();
for row in &all_history[tail_start..] {
    replay_inputs.extend(vault_messages::row_to_input_items(&row.role, &row.content_json));
}

// 3) compaction summary 仍走前置注入(runtime_context_messages),作为 messages
//    数组里的 user 消息;但因 replay_inputs 已是完整 input,改为:
//    messages = runtime_context_messages(&handoff_summaries);  // 仅 summary
//    replay_inputs 承载全部 user/agent/tool 痕迹。
//    (codex EnvironmentContext/UserInstructions 映射见 §7.5)

// 4) 孤儿 function_call 校验: 收集所有 tool_result 的 call_id,
//    从 replay_inputs 删除无配对 output 的 function_call(防 orphan)。
let answered: HashSet<&str> = replay_inputs.iter()
    .filter(|v| v.get("type").and_then(|t|t.as_str()) == Some("function_call_output"))
    .filter_map(|v| v.get("call_id").and_then(|c|c.as_str())).collect();
replay_inputs.retain(|v| {
    if v.get("type").and_then(|t|t.as_str()) == Some("function_call") {
        v.get("call_id").and_then(|c|c.as_str()).map_or(false, |id| answered.contains(id))
    } else { true }
});

if replay_inputs.is_empty() && handoff_summaries.is_empty() {
    anyhow::bail!("run_chat_reply called with no replayable history");
}
```

**每 iteration request 构造（替换 `agent/mod.rs:253-271`）：**

```rust
let mut request_inputs = replay_inputs.clone();   // 稳定前缀
// 唯一允许的末尾瞬时 hint(只在触发的那 iteration append,不破前缀):
if let Some(plan_input) = active_plan_reminder_input(..., iteration, ...).await? {
    request_inputs.push(plan_input);              // 降频,见 §7.3
}
// web guard(:267-271) 删除,纪律下沉 system prompt(见 §7.2)。
let req = ChatRequest {
    messages: messages.clone(),                   // 仅 compaction summary 前置
    additional_inputs: request_inputs,            // = replay_inputs(+尾部瞬时 hint)
    ...
};
```

### 6.4 turn 末持久化（`agent/mod.rs:823-843` 微调）

```rust
// 若 final_text 在 dispatch 中已作为 mid-iteration 文本落库(6.2 A),
// 不重复落。否则按现状落一行 agent 文本。
let msg_seq = if has_content && !mid_iteration_text_persisted_as_final {
    let seq = vault_messages::insert(pool, user_id, session_id, "agent",
        &json!({"type":"text","text": final_text}), None).await?;
    completed_message_seq = Some(seq);
    Some(seq)
} else { ... };
```

### 6.5 retry / cancel / finalize 适配

- **provider retry（`agent/mod.rs:539-570`）：** retry 发生在 stream 阶段、dispatch 落库之前，天然不重复落痕迹。`full_text.truncate`（`:551`）只影响 SSE 预览，不影响已落库内容。保持现状。
- **cancel（`agent/mod.rs:388`）：** `break 'turns` 后已落库的 tool 痕迹天然保留，下次同 session reply 自动重建——这正是 append-only 的可恢复性。无需额外处理。
- **`finalize_after_tool_budget`（`agent/mod.rs:1590-1663`）：** 签名去掉 `session_state_inputs` 参数（`:212`），改收 `replay_inputs: &[Value]`（`:213` 调用处同步）。budget note 仍作为一次性末尾 item append。

---

## 7. context 构建重写

### 7.1 删除 `build_session_state_inputs` 全部 tool 证据/web 活动回灌

**删除（`agent/mod.rs`）：**
- `build_session_state_inputs`（`:917-984`）整个函数 + `:177` 赋值 + `:253` clone。
- `format_tool_run_for_state`（`:986-999`）、`tool_evidence_guidance`（`:1001-1003`）、`web_search_budget_guidance`（`:1005-1007`）、`format_web_search_for_state`。
- `build_recent_web_search_guard_input`（`:1009-1016`）、`recent_web_search_guard_input_from_events`（`:1018-1046`）、`format_web_search_for_guard`（`:1048+`）+ `:267-271` 调用。
- 上述函数的单测。

**理由：** append-only 队列里已有完整 `function_call`/`function_call_output` 原样在 input 中，模型自己看得到调过什么、拿到什么。SESSION STATE 的 tool evidence（截断回灌）与 web activity 纯属重复且破 cache。

> **保留（`is_cacheable_tool` / `find_successful_for_session`，`agent/mod.rs:654-666`）：** tool 结果复用缓存逻辑与 prompt 无关，驱动 `:654` 的 dispatch 短路，保留。`is_refresh_sensitive_tool`/`is_stateful_tool` 等判定逻辑保留。

### 7.2 规训 guidance 下沉静态 system prompt（保 cache，一次性）

把跨 turn 不变的纪律移入 `crates/gateway/harness/discipline.md`（`harness.rs` `include_str!` 静态注入，`build_system_prompt` 一次性带上、走 cache）：
- `tool_evidence_guidance`（复用策略）+ `web_search_budget_guidance`（搜索预算）→ 新增 "§ Evidence reuse & search budget"。
- web guard 的「不重复打开同一 URL/PDF；优先 find-in-page/官方源；SEO 视为弱线索」→ 同节。
- `format_active_plan_state_guidance`（`agent/mod.rs:1735`，收口前别留假 in_progress）→ 新增 "§ Plan hygiene"。

### 7.3 plan reminder：保留但降频，只在触发的那 iteration append 末尾

- 删除 SESSION STATE 里的 plan 段（随 7.1 删 `build_session_state_inputs`）。`update_plan` 的 `function_call_output` 随队列原样回放，模型每 iteration 都能看到最新 plan。
- `active_plan_reminder_input`（`:1672`）/`plan_reminder_tone`（`:1698`）/`format_plan_reminder`（`:1720`）**保留**，但只在需要催更的那一 iteration append 一条到 `request_inputs` 尾部（append-only 容忍偶发末尾提醒，只要不是每 iteration 都变）。固定不变的 plan hygiene 纪律移入 discipline.md（§7.2）。

### 7.4 工具输出控量：拆成「对话时」与「compaction 时」两个独立层面

**先立一条贯穿全方案的设计准则:**

> **确定性的需求,必须在确定性的层面(代码/工具实现)满足,绝不架在 non-deterministic 的 LLM agent 上、靠它「记得」摆某个使用姿势。** —— 「工具输出可能过大、要控量」是确定性需求,所以它是**工具实现的责任**,不是 agent 的责任;不存在「输出太大就请 agent 记得走 subagent」这种无法 enforce 的约定。`delegate_research`(subagent)是 agent/用户想并行或隔离上下文时**主动选用的能力**,与工具输出控量正交,绝不作为某工具的前置依赖。

过去 `compact_*` 把两个层面混在一处、且用了不可逆的语义改写。本方案拆开:

| 层面 | 时机 | 处理 | 章节 |
|---|---|---|---|
| **A. 进 context** | 工具返回、要塞进 LLM 时 | 单条输出控量 | 本节 |
| **B. compaction** | 压缩历史时 | 老工具降级 name+args | §8.4 |

**层面 A —— 对话时单条工具输出进 context(主抄 Claude Code,codex 兜底):**

榜样对比:CC 是「每个工具在实现层自管输出量 + 可参数化/分页取更多」(Read 2000 行+offset、Grep head_limit);codex 是「统一 `tool_output_token_limit=16000` 一刀切砍尾」。我们的工具是**领域专用、参数化的数据查询**(get_financials 可指定期数、get_candlesticks 可指定范围),天然契合 CC 的「要更多就调参数重查」,而非 codex「截了就没法继续取」。所以**主抄 CC,codex 上限作兜底**:

1. **工具实现层直接返回适量数据(确定性):** 每个数据工具按领域知识在自己实现里决定返回什么(几期/哪些字段/多少行),要更多由模型调参数重查。**废弃 `compact_tool_output_for_model` 及全部 `compact_*` 语义改写函数(`agent/mod.rs:1216-1481`)**——它们在「工具返回完整 → 再改写一版喂模型」之间制造割裂(落库完整、喂模型改写、两者不一致),且丢中间/删 URL 不可逆。改造后:**工具返回什么 = 进 context 什么 = 落库什么,三者一致。**
2. **codex 式统一兜底上限(确定性):** 设 `TOOL_OUTPUT_BYTE_LIMIT`(默认 ~24K,`LEEK_TOOL_OUTPUT_BYTE_LIMIT` 可覆盖),任何工具单条输出超限就**砍尾截断**(逐字保留前半)+ 明确标注:
   ```rust
   fn cap_tool_output(output: &str) -> String {
       let limit = tool_output_byte_limit();
       if output.len() <= limit { return output.to_string(); }
       format!("{}\n\n[输出已截断至 {} 字节;完整版在 vault.tool_call_runs / 前端卡片,或用更窄参数重新查询。]",
               preview(output, limit), limit)
   }
   ```
   纯兜底,防任何工具(尤其忘了控量的)单条爆 context。砍尾 ≠ 语义改写:逐字保留前半、明确告知截断,模型知道还有更多、知道怎么取。
3. **截断版落库即定、append-only 不变(保 cache):** `cap_tool_output` 在工具返回那一刻执行一次,结果即 `output_str`、即落库(§5.2)、即回放,之后永不再变。
4. **超大数据(财报全文 / SEC filing):** 由工具**自身在代码层**处理(分段/摘要/返回结构化要点),不依赖 agent 走 subagent。

> **独立性:** 层面 A 工作量在「逐个审视工具返回量 + 删 `compact_*` + 接 `cap_tool_output`」,**相对独立于核心 append-only 改造**,作为核心(§11 Step 1-9)之后的 Step 10,不阻塞主线。核心改造期间 `tool_result` 一律存原始 `output_str`(已是保真);层面 A 落地后工具自身返回就适量、兜底上限再保险。

### 7.5 初始 context 映射（codex build_initial_context）

按三层资产架构对齐 codex 的 UserInstructions → EnvironmentContext 顺序（一次性 append、之后不变）：
- **leek harness** = 顶层 `instructions`（`build_system_prompt`，`llm/openai_responses.rs:49`）。
- **corpus runtime kernel + env**（当前日期 2026/市场状态）= 初始 context，作为 `replay_inputs` 最前的稳定项（本期可继续放在 system prompt，保持最小改动）。
- **vault mandate** = UserInstructions item。
- `build_request_body`（`llm/openai_responses.rs:69-75`）拼接顺序**不改**：messages 在前、`additional_inputs` append 末尾、`prompt_cache_key`（`:124-138`）固定 `leek:{model}:main-agent` 不变。

---

## 8. compaction 改造

### 8.1 触发：双触发(pre-turn 必须 + mid-turn 必须) + 阈值动态化

**阈值动态化(必须,修问题 4):** 废弃硬编码 380K 和「400K」假设,改 `模型 max_context × 90%`,引入「模型 → max_context」映射:
```rust
// 新 llm/model_limits.rs(或 agent/mod.rs 内)
fn model_max_context(model: &str) -> u32 {
    match model {
        m if m.starts_with("gpt-5.5") => 272_000,  // codex backend 实测上限
        m if m.starts_with("gpt-5")   => 272_000,
        _ => 272_000,                                // 未知模型保守默认
    }
}
fn auto_compact_threshold(model: &str) -> i64 {
    std::env::var("LEEK_AUTO_COMPACT_THRESHOLD").ok().and_then(|s| s.parse().ok())
        .unwrap_or((model_max_context(model) as f64 * 0.90) as i64)  // 272K → ~245K
}
```
- 删 `api/messages.rs:23` 的 `AUTO_COMPACT_THRESHOLD_DEFAULT = 380_000` 与「400K」注释。
- 将来加 provider/model:只在 `model_max_context` 加一行各自上限,阈值自动跟随。

**pre-turn(必须,已存在,改用动态阈值):** `api/messages.rs:74-95` 逻辑保留,`auto_compact_threshold()` 改为按当前 model 返回动态值。

**mid-turn(必须,新增 —— 刚需不是可选):** 一个长 turn 光 tool 调用就能在「两次提问之间」内部把 context 顶爆,只靠 pre-turn 拦不住。在 `agent/mod.rs:201` 的 `'iterations` 循环顶部、每次 `provider.chat` 之前:
```rust
// 读本 turn 最近 Usage.input_tokens(:483)
if last_input_tokens >= auto_compact_threshold(&request_model) {
    compact_session_tail(&pool, &user_id, &session_id /*, keep_recent_turns */).await?; // 同步,非 spawn
    replay_inputs = rebuild_replay_inputs(&pool, &user_id, &session_id).await?;
    continue 'iterations;
}
```
**死锁规避(关键工程点):** 当前 reply 已持 `active_replies` 互斥(`api/sessions.rs:213`),mid-turn 压缩**不能走** `api_sessions::start_compaction`(它会再抢同一把锁 → 自我死锁)。必须走**内部直连** `compact_session_tail`(直接调 `compact::summarize_session` + 落 `compaction_summary` 行,不经 `active_replies` 调度)。

### 8.2 与 append-only 共存的边界设计

- **边界锚不变：** `run_compaction`（`api/sessions.rs:331-335`）和 `run_chat_reply`（`agent/mod.rs:117-128`）的 `rposition(role=="compaction_summary")` 切 tail 逻辑保留。append-only 后 tail 自然含 `assistant_tool_calls`/`tool_result` 行。
- **`messages_removed`（`api/sessions.rs:340`）：** `history.len()` 现在会把 tool-trace 行也计入（合理，它们确实被移出 context）。
- **compaction 后回放：** 从新 `compaction_summary` 之后开始，旧 tool-trace 行留 DB 但不进 input（`agent/mod.rs:117-128` tail 逻辑天然处理）。
- **`render_transcript`（`compact.rs:90-113`）必须增强：** 当前 `_ => continue`（`:107`）跳过所有 tool 行,会让压缩摘要丢失 tool 证据。改为按 §8.4 的「工具 N-turn 降级」渲染 tool 行:近 N turn 渲染细节(call+args+output)、>N turn 只渲染 `name+args`,让 summarizer 直接读到 tool 痕迹(不再依赖 `load_compaction_supporting_context`（`api/sessions.rs:386`）旁路截断版)。这是「压缩后 tool 证据不丢」的**必须项**,不再可选。

### 8.3 审计补全（可选）

`vault_compactions::insert`（`api/sessions.rs:364-377`）当前 `tokens_before`/`tokens_after` 恒 None、`messages_retained` 恒 1。可选改进：`tokens_before` 填 `latest_input_tokens`，`messages_retained` 填实际保留尾条数。列已 nullable，无需改 schema。**本期可不做。**

### 8.4 compaction 时的工具 N-turn 降级(点 1)

> **再次强调:这只发生在 compaction 这个动作里,正常对话回放绝不裁剪工具数据。** 若正常对话也按 turn 窗口裁老工具,context 前缀就变了 → prompt cache 全废,反而把本方案要解决的问题又制造一遍。正常对话 = append-only 队列完整原样回放,所有 tool 痕迹只增不减。

工具数据是 context 里最占空间、又最易过时的部分。compaction 渲染待压缩 transcript 时,按工具调用所属 turn 的新旧分两档(`LEEK_TOOL_KEEP_TURNS` 默认 6):
- **最近 N 个 turn 内**的工具调用:渲染**完整细节**(call name + args + output,output 受 §7.4 兜底上限约束),让 summarizer 保留近期精确证据。
- **N 个 turn 之前**的工具调用:**降级成一行 `name + args` 摘要,丢 output**,例如 `[earlier tool: get_financials(ts_code=600519, period=annual)]`。模型仍保有「早先查过茅台年报」这个索引,但不为过时的原始数字付 token。

实现:`render_transcript`(`compact.rs:90-113`)按行的 turn 归属(seq 顺序推断:每个 `user` 行开启一个新 turn) + role(`assistant_tool_calls`/`tool_result`)分派渲染。降级只影响**喂给 summarizer 的 transcript 文本**与压缩后摘要;DB 里的原始 tool 痕迹永久保留、前端永远能查。

> **为何不用 `/responses/compact`：** codex 加密路径返回不可读 `type=compaction` encrypted_content item，绑定 codex backend 特定行为，且我们已有可工作的本地 LLM 蒸馏路径（`compact.rs:242` `summarize_session`）。本期沿用本地蒸馏，只对齐触发/边界语义，降低风险。可作为后续独立调研（open_question）。

---

## 9. 迁移与向后兼容

**老 session 数据 = 测试数据,直接可删,不做任何兼容回填。** 改造前的 session 只有 `user`/`agent`/`compaction_summary` 行、无 tool 痕迹,且都是开发期测试数据、无保留价值。上线时:

- **首选:清空老数据。** 上线前清空 `messages`/`events`/`tool_call_runs` 等(或直接换新 `vault.db`),让所有 session 从干净的 append-only 起步。**砍掉一整套「老 session 降级兼容」逻辑**——无需精简版 SESSION STATE fallback、无需回填脚本,方案更简单。
- **即便不清:零崩溃兜底。** `row_to_input_items` 对老行天然正常(user→user、agent→assistant、summary→跳过),回放不报错,只是旧 turn 无 tool 痕迹——既然是测试数据,无所谓。

**回滚策略:**
1. migration 0009 纯加索引,回滚只需 `DROP INDEX idx_messages_replay`(不动数据)。
2. 代码回滚:revert `agent/mod.rs` + `vault/messages.rs` + `list_handler` 即可。已落库的 `assistant_tool_calls`/`tool_result` 行在旧代码下被 `agent/mod.rs:130-145` filter 忽略(只认 user/agent),**旧代码读新数据不崩溃**——天然向后兼容。
3. 前端:`list_handler` 过滤(§5.3 `list_for_ui`)若漏改,新 role 会被前端当 user 渲染(`LiveChat.tsx:970` else→user)→ 空白气泡。这是**最高优先级风险**,必须随后端同步上线。

---

## 10. 测试与验证

**单元测试（`cargo test`）：**
1. `row_to_input_items` round-trip：写入 function_call+output → list → 还原，断言 call_id/arguments/output byte-equal。
2. 孤儿 call 校验：构造「有 function_call 无 output」的 tail，断言重建后该 function_call 被剔除。
3. dispatch 落库：mock provider 返回 1 个 function_call，断言 messages 表新增 `assistant_tool_calls`(1) + `tool_result`(1) 行、seq 递增、call_id 一致。
4. 多 call iteration：mock 返回 2 个并发 call，断言 `assistant_tool_calls.items` 长度=2、2 行 tool_result、每个 `tool_result.seq > assistant_tool_calls.seq`。
5. retry + tool 组合：mock 第一次 provider error、retry 后返回 function_call，断言痕迹只落一次（不重复）。
6. mid-iteration text 顺序：mock 一个 iteration 既输出文本又触发 tool，断言落库顺序 agent → assistant_tool_calls → tool_result。
7. `list_for_ui` 过滤：插入混合 role，断言只返回 user/agent/compaction_summary。
8. 删除 `build_session_state_inputs`/web guard/`compact_*` 后对应单测同步删除，`cargo test` 编译通过。

**集成 / 手动验证：**
- **tool 痕迹跨 turn 可见：** 连续 2 个 turn，第二 turn 问「刚才查的市值是多少」，模型能答出 → 证明 tool 痕迹持久且原样回放。`SELECT seq,role FROM messages WHERE session_id=? ORDER BY seq` 应见 `user → assistant_tool_calls → tool_result → ... → agent`。
- **prompt cache 命中：** 同一 session 连发两条消息，对比 `llm_usage_log` 的 `cache_read_tokens`（`llm/mod.rs:146` Usage）应从接近 0 显著上升。进阶：抓两次 provider 请求 body，断言 `input[..N]` 前缀 byte-identical。
- **UI 无回归：** 带 tool 的 turn 后刷新前端，聊天区只显示 user/agent/compaction_summary 气泡、无空白 user 气泡；tool 调用仍通过 events（`tool_call` 事件）正常展示。
- **compaction 共存：** 对含 tool 痕迹的 session 跑 `/compact`，`compaction_summary` 写入后再发新消息，replay input 不含边界之前的 tool_result 行；`session_compactions.messages_removed` 含 tool-trace 行数。

---

## 11. 分步执行 workflow

> 每步独立可验证，按序推进。标 **[必须]** / **[可选]**。

**Step 1 [必须] — migration 0009**
- 改：新建 `crates/gateway/migrations/0009_conversation_queue.sql`（§5.1 DDL）。
- 验证：`cargo sqlx migrate run`；查 `idx_messages_replay` 存在；`messages` 行数不变。

**Step 2 [必须] — vault 反序列化辅助**
- 改：`vault/messages.rs` 新增 `row_to_input_items`、`list_for_ui`。
- 验证：`cargo test` 跑 round-trip 单测（测试 1、7）通过。

**Step 3 [必须] — 术语 rename**
- 改：`agent/mod.rs` 按 §4 清单机械 rename（先做这步，避免后续改动与 rename 冲突）。
- 验证：`cargo build` 通过；`grep -n '\bturn\b'` 仅剩注释/label/event key/stop_reason。

**Step 4 [必须] — dispatch 落库**
- 改：`agent/mod.rs` 新增 `persist_tool_calls`/`persist_tool_result`（§6.1）；替换 `:769-783` push 为落库（§6.2）；改 `:782` 用原始 output。
- 验证：`cargo test` 跑落库单测（测试 3、4、5、6）通过；手动跑带 tool 的 turn，查 messages 表有 tool-trace 行。

**Step 5 [必须] — input 回放重建**
- 改：`agent/mod.rs:111-152` + `:253-271` 按 §6.3 重写为 `replay_inputs` 驱动；删 `:182` `additional_inputs` 旧用法（改名为 `replay_inputs`）；孤儿 call 校验。
- 验证：测试 2 通过；手动跑 2 turn 引用测试（tool 痕迹跨 turn 可见）。

**Step 6 [必须] — 删 SESSION STATE / web guard，纪律下沉**
- 改：`agent/mod.rs` 删 §7.1 列出的函数 + 调用 + 单测；`harness/discipline.md` 加 §7.2 两节；plan reminder 改降频末尾 append（§7.3）。
- 验证：`cargo build` + `cargo test` 通过（删测试后无残留引用）；`finalize_after_tool_budget` 签名同步。

**Step 7 [必须] — list_handler UI 过滤**
- 改：`api/messages.rs:316` `list_handler` 改用 `vault_messages::list_for_ui`（或 `rows.retain(...)`）。
- 验证：带 tool 的 turn 后刷新前端，无空白 user 气泡。

**Step 8 [必须] — compaction 改造(阈值动态化 + 双触发 + 工具降级)**
- 改:(a) 阈值动态化(§8.1)——删 `api/messages.rs:23` 硬编码 380K,加 `model_max_context` 映射(gpt-5.5=272K)+ `auto_compact_threshold(model)=max_context×90%`;pre-turn 改用动态阈值。(b) mid-turn 触发(§8.1)——`'iterations` 循环顶部按 `Usage.input_tokens ≥ 阈值` 同步压缩,走内部 `compact_session_tail` 避免 `active_replies` 自我死锁。(c) 工具 N-turn 降级 + `render_transcript` 增强(§8.4):近 N(默认6)turn 工具渲染细节、>N turn 降级 `name+args`。(d) 切 tail 边界复核(`api/sessions.rs:331-340`、`agent/mod.rs:117-128` 无需改)。
- 验证:`LEEK_AUTO_COMPACT_THRESHOLD=2000` 下,(i) 单 turn 内大量 tool 调用触发 mid-turn 压缩(turn 内见 `compaction.completed`、无死锁);(ii) 压缩后 replay 不含边界前 tool_result;(iii) 压缩摘要里近 N turn 有 tool 细节、更老的只剩 `name+args`。

**Step 9 [必须] — prompt cache 验证**
- 验证:同 session 连发两条,`cache_read_tokens` 显著上升;抓请求 body 比对前缀 byte-identical。

**Step 10 [必须·可后置] — 层面 A 工具输出控量(独立于核心,不阻塞主线)**
- 改:删全部 `compact_*` 语义改写函数(`agent/mod.rs:1216-1481`)及 `:719` 调用;新增 `cap_tool_output`(codex 式砍尾兜底上限,§7.4);逐个审视数据工具返回量,让工具自身返回适量。确认前端 UI artifact 不依赖 `model_output`。
- 验证:`cargo test` 通过;带大输出工具的 turn,`tool_result` 落原始或砍尾标注版、UI 卡片仍完整;token 预算实测。

**Step 11 [可选] — compaction 审计补全**
- 改:§8.3 的 `tokens_before`/`messages_retained` 填真实值。
- 验证:compaction 后查 `session_compactions` 字段非空。

---

## 12. 验收 checklist

- [ ] migration 0009 跑过，`idx_messages_replay` 存在，老数据零损。
- [ ] `cargo build` + `cargo test` 全绿，无残留 `build_session_state_inputs`/web guard/相关单测引用。
- [ ] `grep '\bturn\b' agent/mod.rs` 仅剩注释/`'iterations` label/event payload key `"turn"`/stop_reason 字面量；变量全部 `iteration`。
- [ ] 带 ≥2 tool 的 turn 后，`SELECT seq,role FROM messages ORDER BY seq` 见 `user → (agent?) → assistant_tool_calls → tool_result+ → agent`，每个 `tool_result.seq > assistant_tool_calls.seq`。
- [ ] `tool_result.output` 存的是原始 `output_str`（非 `model_output`），与 `tool_call_runs.result_json.output` 一致。
- [ ] 连续 2 turn，第二 turn 能引用第一 turn 的 tool 结果（跨 turn 痕迹可见）。
- [ ] 同 session 连发两条，`cache_read_tokens` 显著上升（前缀稳定）。
- [ ] 前端聊天区只显示 user/agent/compaction_summary，无空白 user 气泡；tool 调用经 events 正常展示。
- [ ] 老 session 续聊回放不报错（降级无 tool 痕迹，可接受）。
- [ ] 含 tool 痕迹的 session compaction 后，replay input 不含边界前 tool_result 行，无 orphan call_id 报错。
- [ ] 回滚演练：revert 代码后旧代码读新数据（tool-trace 行）不崩溃。
- [ ] auto-compact 阈值改为 `max_context × 90%` 动态值(gpt-5.5 → ~245K)、无硬编码 380K;`model_max_context` 映射存在,新增模型只加一行。
- [ ] mid-turn 压缩:单 turn 内 input 超阈值能触发同步压缩、不死锁、压缩后 turn 继续跑完。
- [ ] compaction 摘要:近 N turn 工具有细节、>N turn 只剩 `name+args`;DB 原始 tool 痕迹仍在、前端可查。
- [ ] (Step 10 后) `compact_*` 已删、`cap_tool_output` 砍尾兜底生效、UI 卡片完整;工具返回=进context=落库三者一致。

---

**改动文件清单（精确）：** 新建 `crates/gateway/migrations/0009_conversation_queue.sql`；改 `crates/gateway/src/vault/messages.rs`、`crates/gateway/src/agent/mod.rs`（dispatch/回放/rename/删 SESSION STATE）、`crates/gateway/src/api/messages.rs:316`（list_handler）、`crates/gateway/harness/discipline.md`（纪律下沉）；复核 `crates/gateway/src/api/sessions.rs:314-384`、`crates/gateway/src/agent/compact.rs:90-113`、`crates/gateway/src/llm/openai_responses.rs:17-97`（不改）；可选改 `crates/gateway/src/agent/mod.rs:1216-1481`（`compact_*` 去留）。