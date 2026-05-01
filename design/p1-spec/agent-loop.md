# P1 Spec — Agent Loop（自实现 Harness）

> Main agent 的核心循环、subagent 调度、task lifecycle 与 loop 的交互、scratchpad / thinking 持久化、reasoning DAG 节点生成时机。

依赖：[ADR-0005](../decisions/0005-self-implemented-harness.md)（自实现 harness）、[ADR-0010](../decisions/0010-single-agent-coordinator-subagent.md)（单 agent + subagent map-reduce）、[`interaction-model.md`](../interaction-model.md)（task lifecycle）、[`tools.md`](tools.md)、[`llm-provider.md`](llm-provider.md)、[`data-schema.md`](data-schema.md)。

## 1. 设计目标

1. **Task-driven**：harness 是任务驱动的，不是 message 驱动的——每个 task 一个 loop instance，loop 在 task lifecycle 内运转
2. **完全可控**：上下文裁剪 / thinking 模式 / tool calling / 中断 / 续跑全部由 leek 自己决定
3. **可中断**：每个 yield point 都能接收用户的 control 命令（追加约束 / 重 scope / 中断）
4. **可恢复**：中断 / cancel 后，所有状态持久化在 vault；下次启动可以从断点继续
5. **可观测**：loop 的每个 phase 都推事件，前端能看到完整执行过程
6. **可移植**：核心 loop 不依赖某个 provider 的协议特性

## 2. 主循环结构

```rust
// crate::agent::loop

pub struct AgentLoop {
    // 不变量
    user_id: String,
    session_id: String,
    task_id: String,
    
    // 注入
    llm_registry: Arc<LlmRegistry>,
    tool_registry: Arc<ToolRegistry>,
    event_bus: Arc<EventBus>,
    vault: Arc<Vault>,
    corpus: Arc<Corpus>,
    
    // 状态
    state: AgentLoopState,
}

pub struct AgentLoopState {
    pub task: Task,
    pub charter: TeamCharter,
    pub messages: Vec<Message>,           // 完整 message 历史（含 tool_use / tool_result）
    pub scratchpad: Scratchpad,           // agent 的"私人笔记"
    pub current_iteration: u32,
    pub max_iterations: u32,
    pub tokens_used_so_far: u32,
    pub control_inbox: Vec<ControlMessage>, // 用户中途的 control 命令
    pub status: LoopStatus,
}

pub enum LoopStatus {
    Running,
    AwaitingUser,                          // 调用了 clarify.ask_user
    AwaitingSubagents(Vec<String>),        // run_id list
    Interrupted,
    Done,
    Failed(String),
}

pub enum ControlMessage {
    AppendConstraint(String),               // 追加约束
    Rescope { new_goal: String, new_constraints: Constraints }, // 重 scope
    Interrupt,                              // 中断
    SkipCurrentStep,                        // 跳过当前步骤
    PinPanel(String),                       // 钉住 panel
    UserResponse(String),                   // 在 awaiting_user 时用户回答
}
```

## 3. 完整循环算法

```
fn run(task: Task) -> Deliverable {
    // 1. 初始化
    let state = init_state(task);
    persist(state);

    while state.status == Running {
        // 2. 处理 control inbox（每轮开始前）
        process_control_messages(&mut state);
        
        if state.status == Interrupted {
            mark_task_cancelled(state.task_id);
            return;
        }

        // 3. 构建 LLM context
        let request = build_chat_request(&state);

        // 4. 调用 LLM 流式接收
        emit(Event::AgentThinkingStart);
        let mut stream = llm_registry.chat(request).await?;
        let outcome = consume_stream(&mut state, &mut stream).await?;

        // 5. 解析 outcome
        match outcome {
            // 5a. LLM 输出了 tool_use → 执行工具
            Outcome::ToolCalls(calls) => {
                let results = dispatch_tools(&mut state, calls).await;
                state.messages.push(Message::ToolResults(results));
                state.current_iteration += 1;
            }

            // 5b. LLM 输出了最终回复（无 tool_use）→ 看是否要交付
            Outcome::FinalReply { text, structured } => {
                if let Some(deliverable) = detect_deliverable(&state, text, structured) {
                    finalize_deliverable(deliverable);
                    state.status = Done;
                } else {
                    // 没在 deliverable 形态 → 当成普通 chat reply
                    write_chat_reply(text);
                    state.status = Done;
                }
            }

            // 5c. agent 调了 clarify.ask_user → 暂停
            Outcome::AwaitingUser => {
                state.status = AwaitingUser;
                set_task_status(state.task_id, "awaiting_user");
                // loop 退出，等用户回答 → 新一轮 schedule
            }

            // 5d. agent 调了 subagent.spawn 多个并行 → 等待
            Outcome::AwaitingSubagents(run_ids) => {
                state.status = AwaitingSubagents(run_ids);
                // 不退出 loop；await 所有完成
                let merged_results = await_all_subagents(run_ids).await;
                state.messages.push(Message::SubagentResults(merged_results));
                state.status = Running;
            }

            Outcome::Error(e) => {
                state.status = Failed(e.to_string());
                set_task_status(state.task_id, "failed");
                return;
            }
        }

        // 6. 安全停止条件
        if state.current_iteration >= state.max_iterations {
            emit_warning("max iterations reached");
            request_self_summary_or_clarify(&mut state);
        }
        if state.tokens_used_so_far >= state.token_budget {
            emit_warning("token budget exceeded");
            request_self_summary_or_clarify(&mut state);
        }

        persist(state);
    }
}
```

## 4. Context Build 详细

每个 LLM 调用前都重新构建 chat request。**Context 不是简单 append**——需要根据当前 task 与 history 智能组织。

```rust
fn build_chat_request(state: &AgentLoopState) -> ChatRequest {
    // 1. System prompt 部分
    let system = build_system_prompt(&state.task, &state.charter);

    // 2. Messages
    let messages = build_messages(state);

    // 3. Tools
    let tools = registry.list_for(&state.task.expected_deliverable);

    // 4. Options
    let options = ChatOptions {
        max_tokens: Some(8192),
        thinking: Some(determine_thinking_config(&state)),
        cache_strategy: CacheStrategy::Aggressive,
        ..Default::default()
    };

    ChatRequest { messages, system_prompt: Some(system), tools, options }
}
```

### 4.1 System Prompt 构成

```
[Persona]
你是 L.E.E.K (老韭菜) 的核心研究 lead。用户是基金经理，你是他指挥的研究团队。
你为他做严肃投研——你的输出会变成实际投资决策。
请保持专业、严谨、有立场。不要含糊其辞，不要规避具体建议。

[Task Charter（团队工作章程）]
<注入 team_charter.charter_json>
- 风格：{style}
- 硬约束：{hard_limits}
- 软偏好：{soft_preferences}
- 工作风格：{work_style}

[Current Portfolio Snapshot]
<最近 holdings 快照，前 30 行>

[Active Watchlists]
<watchlists 摘要>

[Tools Available]
你可以调用以下工具（详见 tool definitions）...

[Subagent Capability]
你可以通过 subagent.spawn 调度临时小组完成可分解的子任务。
适合 spawn 的场景：
  · 多标的并行调研（parallel）
  · 大量文档要消化（避免主 context 爆）
  · 需要"clean room"避免 anchoring 的子推理
  · 试探性参数 sweep
spawn 时给 subagent 明确的 scope.goal、allowed_tools、return_schema。
拿到结果后由你 merge。subagent 不能写 vault，所有写入都通过你。

[Reasoning Style]
对于复杂任务请显式分解：先用 `plan.draft` 工具列出步骤（产生 DAG 上的 plan 节点），
执行过程中可以用 `plan.update` 调整。绝大多数其他节点会从 tool calls 自动生成
——不需要你显式 add_node。当推理路径有分支或重要观察时，用 `reasoning.note` 工具
显式记一笔（产生 thinking 节点）。

[Mandate Enforcement]
当你输出 decision draft 时，**必须**做 mandate check：
  · 如果违反 hard_limits → 自动调整建议或在 rationale 解释
  · 如果触发 soft_preferences 的警告 → 在 mandate_check 字段显式列出

[Citation Discipline]
当你引用 corpus 中的概念或原则时，把对应的 wikilink_id 放进 corpus_refs。
这会触发 CorpusBrain 的激活动效，是产品的 signature 体验之一。

[Output Discipline]
你的工作过程在 canvas Reasoning DAG 上展开（详见 frontend/panels.md）——每次 tool call 自动产生一个
typed observation 节点（quote / candles / cites / news / cmptable / valuation / subagent / ...）。
chat 主轴只放最终简短回复，**绝不**把中间过程写到 chat。

最终产出形态按 task.expected_deliverable 区分：
- `decision_draft` → 调 `decision.draft` 工具，产出 decision_draft 节点（含 mandate check）
- `review` → 调 `review.draft` 工具，产出 review_draft 节点
- `comparison` → 串多个 quote / cmptable / candles 节点，最终一个 final_reply 文本节点收尾
- `research_brief` / `morning_brief` → 多个 typed observation 节点 + 一个 final_reply 收尾
- `free_form` → 你自己判断节点类型组合
- 闲聊 / 澄清 / chat thread 内追问 → **不开 task**，直接 chat 文本回复；不产 DAG 节点

---

[Current Task]
Title: <task.title>
Goal: <task.goal>
Constraints: <task.constraints_json>
Expected deliverable: <task.expected_deliverable>
Priority: <task.priority>
```

System prompt 整体设计为**长且稳定**——便于 prompt caching 命中。每次调用只 append 新 message，system 部分不变。

### 4.2 Messages 拼装

按以下顺序：

1. **历史 user/agent message**（来自 `vault.messages`，按 seq 升序，仅当前 task）
2. **历史 tool_use / tool_result**（来自当前 loop iteration 的 messages）
3. **历史 subagent 结果**（如果有）
4. **最新一条 user message**（如果是 awaiting_user 收到的回答）

注意：
- 如果一个 task 是从 cron / agent_proposed 创建的（没有 user message），第一轮的 user message 是合成的（自动生成 "执行任务: {task.goal}" 形式）
- 长 thread 的 messages 数量大（>30 条）时启用裁剪策略（见 §4.3）

### 4.3 Context 裁剪策略

P1 简化：

- 总 token 预算 = `provider.capabilities.max_context_tokens * 0.7`（留 30% 给 output 和 cache miss）
- 必保留：system prompt + tools + 当前 task 的 active deliverable 状态 + 最近 3 轮（含 tool_use/result）
- 可裁剪：远 history 的 tool_result（保留摘要而非全文）、已 close 的 panel 状态

P2 增强：semantic relevance 排序、agent 自己决定保留什么。

裁剪时对用户透明：在 reasoning DAG 加一个 `context_pruned` 节点。

## 5. 流式接收与 Outcome 解析

```rust
async fn consume_stream(
    state: &mut AgentLoopState,
    stream: &mut BoxStream<LlmEvent>,
) -> Result<Outcome, AgentError> {
    let mut accumulated_text = String::new();
    let mut accumulated_thinking = String::new();
    let mut tool_calls: Vec<PendingToolCall> = vec![];
    let mut current_tool_call: Option<&mut PendingToolCall> = None;
    let mut stop_reason = None;

    while let Some(event) = stream.next().await {
        let event = event?;

        match event {
            LlmEvent::Start { .. } => {}

            LlmEvent::ThinkingDelta { text } => {
                accumulated_thinking.push_str(&text);
                emit(Event::AgentThinkingDelta { text });
                // thinking 累积到节点边界时（启发式：换行 / 句号）触发 reasoning.add_node
            }

            LlmEvent::TextDelta { text } => {
                accumulated_text.push_str(&text);
                emit(Event::AgentMessageDelta { text });
            }

            LlmEvent::ToolCallStart { id, name } => {
                tool_calls.push(PendingToolCall::new(id.clone(), name.clone()));
                current_tool_call = tool_calls.last_mut();
                emit(Event::ToolCallDetected { id, name });
            }

            LlmEvent::ToolCallArgsDelta { id, delta } => {
                if let Some(c) = &mut current_tool_call {
                    c.partial_args_json.push_str(&delta);
                }
                emit(Event::ToolCallArgsDelta { id, delta });
            }

            LlmEvent::ToolCallEnd { id } => {
                if let Some(c) = &mut current_tool_call {
                    c.finalize_args()?;
                }
                current_tool_call = None;
            }

            LlmEvent::Usage { input_tokens, output_tokens, .. } => {
                state.tokens_used_so_far += input_tokens + output_tokens;
                write_usage_log(state, ...);
                emit(Event::Usage { input_tokens, output_tokens });
            }

            LlmEvent::MessageEnd { stop_reason: sr } => {
                stop_reason = Some(sr);
            }

            LlmEvent::Done => break,
        }

        // 中断检查（每个 event 后检查 control inbox）
        if state.control_inbox.iter().any(|m| matches!(m, ControlMessage::Interrupt)) {
            return Ok(Outcome::Interrupted);
        }
    }

    // 持久化 message
    state.messages.push(Message::Assistant {
        thinking: accumulated_thinking,
        text: accumulated_text.clone(),
        tool_calls: tool_calls.clone(),
    });

    // 推 message_end 事件
    emit(Event::AgentMessageEnd);

    // 决定 outcome
    if !tool_calls.is_empty() {
        // 检查是否含 spawn_subagent 调用 → 特殊处理
        let (subagent_spawns, regular_calls): (Vec<_>, Vec<_>) =
            tool_calls.into_iter().partition(|c| c.name == "subagent.spawn");
        
        if !subagent_spawns.is_empty() && regular_calls.is_empty() {
            // 全是 subagent spawn → 进入 awaiting_subagents
            return Ok(Outcome::AwaitingSubagents(spawn_all_async(subagent_spawns)));
        }
        // 其余 tool calls
        return Ok(Outcome::ToolCalls(tool_calls));
    }

    if accumulated_text.trim().is_empty() && stop_reason == Some(StopReason::EndTurn) {
        return Ok(Outcome::Empty);
    }

    Ok(Outcome::FinalReply {
        text: accumulated_text,
        structured: None,
    })
}
```

## 6. Tool Dispatch

```rust
async fn dispatch_tools(
    state: &mut AgentLoopState,
    calls: Vec<PendingToolCall>,
) -> Vec<ToolResult> {
    // 并行 dispatch（每个 tool 一个 tokio task）
    let futures = calls.into_iter().map(|call| {
        let registry = self.tool_registry.clone();
        let ctx = self.tool_context();
        tokio::spawn(async move {
            let started_at = Utc::now();
            let run_id = persist_tool_call_start(&ctx, &call, started_at);
            emit(Event::ToolCallStart { run_id: run_id.clone(), name: call.name.clone(), args: call.args_json.clone() });

            let result = registry
                .dispatch(&call.name, call.args.clone(), &ctx)
                .await;

            let completed_at = Utc::now();
            let duration_ms = (completed_at - started_at).num_milliseconds() as u64;

            persist_tool_call_complete(&ctx, &run_id, &result, duration_ms);
            emit(Event::ToolCallResult {
                run_id: run_id.clone(),
                result: result.clone(),
                duration_ms,
            });

            // ReasoningDAG 加节点（自动）
            reasoning_add_node_for_tool_call(&ctx, &call, &result);

            ToolResult::from(result, call.id)
        })
    });

    join_all(futures).await
        .into_iter()
        .map(|r| r.unwrap_or_else(|panic_err| ToolResult::error(panic_err)))
        .collect()
}
```

并发上限：默认同时最多 5 个 tool 并行；超出排队（防止打挂下游）。

## 7. Subagent 调度

`subagent.spawn` 是个特殊 tool——它的"返回"包含完整 LLM loop 的执行。流程：

```rust
async fn spawn_subagent(
    main_ctx: &ToolContext,
    args: SubagentSpawnArgs,
) -> Result<SubagentOutput, ToolError> {
    let spec = subagent::specs::lookup(&args.spec_name)?;
    let scope = args.scope;

    // 1. 注册 run
    let run_id = Uuid::new_v7();
    persist_subagent_run_start(&main_ctx, &run_id, &spec, &scope);
    emit(Event::SubagentStarted { run_id, spec_name, scope });

    // 2. 构建 subagent 的独立 context
    let sub_ctx = ToolContext {
        invoker: Invoker::Subagent { run_id: run_id.clone() },
        // 工具 registry 已经知道怎么按 invoker 过滤
        ..main_ctx.clone()
    };

    // 3. 启动独立 LLM loop
    let mut sub_state = SubagentState::new(spec.clone(), scope.clone(), args.input);
    let mut iter = 0;

    while iter < scope.max_turns && sub_state.tokens_used < scope.max_tokens {
        let request = build_subagent_request(&sub_state);
        let stream = llm_registry.chat(request).await?;
        let outcome = consume_subagent_stream(&mut sub_state, stream).await?;

        match outcome {
            SubOutcome::ToolCalls(calls) => {
                let results = dispatch_tools_for_subagent(&sub_ctx, calls).await;
                sub_state.messages.push(Message::ToolResults(results));
                iter += 1;
            }

            SubOutcome::FinalReply { structured } => {
                // structured 必须符合 scope.return_schema
                if !validate_against(&structured, &scope.return_schema) {
                    // agent 输出格式错误 → 再来一轮要它改
                    sub_state.messages.push(Message::User(
                        "Your output didn't match the required schema. Please retry conforming to schema."
                    ));
                    iter += 1;
                    continue;
                }

                let output = SubagentOutput {
                    success: true,
                    result: structured,
                    summary: extract_summary(&sub_state),
                    tokens_used: sub_state.tokens_used,
                    turns: iter as u32,
                    duration_ms: sub_state.elapsed_ms(),
                    error: None,
                };
                persist_subagent_run_complete(&main_ctx, &run_id, &output);
                emit(Event::SubagentCompleted { run_id, output: output.clone() });
                return Ok(output);
            }

            // subagent 不能 spawn 嵌套 → registry 已拒；此处不处理
            // subagent 不能 ask user → registry 已拒
        }
    }

    // 超时 / 超 budget → 强制返回 partial
    let partial = SubagentOutput {
        success: false,
        result: extract_partial(&sub_state),
        summary: format!("Reached max_turns or max_tokens. Partial result attached."),
        tokens_used: sub_state.tokens_used,
        turns: iter as u32,
        duration_ms: sub_state.elapsed_ms(),
        error: Some("budget exceeded".into()),
    };
    persist_subagent_run_complete(&main_ctx, &run_id, &partial);
    emit(Event::SubagentCompleted { run_id, output: partial.clone() });
    Ok(partial)
}
```

### 7.1 Subagent 的 system prompt

每个 subagent spec 有独立的 system prompt template：

```
[Subagent Role]
你是 L.E.E.K 主 agent 调度的临时小组（{spec_name}）。
你的工作范围严格限定在以下 scope：
  Goal: {scope.goal}
  Allowed tools: {scope.allowed_tools}
  Return schema (你的最终输出必须符合):
    {scope.return_schema}

[Constraints]
- 你不能 spawn 其他 subagent
- 你不能写 vault
- 你不能问用户问题（直接做决定或在结果中标记不确定性）
- 当你完成时，输出必须严格匹配 return_schema 的 JSON
- 你的整个执行预算：{max_turns} 轮 LLM 调用，{max_tokens} tokens，{max_duration_sec} 秒

[Context from main agent]
{input.context}

[Parameters]
{input.parameters}

请直接开始工作。
```

### 7.2 并行 spawn

主 agent 一次输出可以包含多个 `subagent.spawn` 调用。loop 检测到全是 spawn 时进入 `AwaitingSubagents` 状态，并行 spawn 所有 → join_all 等待。

注意：tool call 是并行的，但**主 agent 看到 result 必须等所有 spawn 都完成**——这是 map-reduce 的 reduce 阶段约束。如果想要 fire-and-forget 的 spawn，未来可以加 `subagent.spawn_async`，P1 不做。

## 8. Reasoning DAG 自动生成

不需要 agent 主动调 `reasoning.add_node`——大部分节点由 loop 在生命周期事件中自动生成：

| Loop 事件 | DAG 操作 |
|--|--|
| Task 启动 | 加根节点（kind=user_input, title=task.title） |
| LLM 流式输出 thinking 完整段 | 加 thinking 节点 |
| Tool call 启动 | 加 tool_call 节点（连边到上一个节点）|
| Tool call 结果 | 加 observation 节点（连到 tool_call 节点）|
| corpus.search / corpus.read 命中 | 加 corpus_ref 节点 + 推 corpus_node_activated 事件 |
| subagent.spawn 启动 | 加 subagent_branch 节点（特殊 kind） |
| subagent 完成 | 加 subagent_result 节点（含 summary） |
| decision.draft 提交 | 加 decision_draft 节点 |
| 最终回复 | 加 final_reply 节点 |

DAG 状态持久化在 `vault.reasoning_dag_traces`，每次更新（add_node / add_edge）原子写入。

## 9. Control 命令处理

```rust
fn process_control_messages(state: &mut AgentLoopState) {
    while let Some(msg) = state.control_inbox.pop() {
        match msg {
            ControlMessage::AppendConstraint(text) => {
                // 把约束作为新 user message 注入
                state.messages.push(Message::User {
                    content: ContentPart::Text(format!(
                        "[Manager 追加约束]: {}", text
                    )),
                    metadata: { is_control: true },
                });
                emit(Event::ControlAck { kind: "append_constraint" });
            }

            ControlMessage::Rescope { new_goal, new_constraints } => {
                // 更新 task 字段
                state.task.goal = new_goal;
                state.task.constraints_json = serde_json::to_string(&new_constraints)?;
                update_task_in_vault(&state.task);
                
                // 注入提示
                state.messages.push(Message::User {
                    content: ContentPart::Text(format!(
                        "[Manager 重新 scope]\n新目标: {}\n新约束: {:?}\n请重新规划执行计划。",
                        state.task.goal, new_constraints
                    )),
                });
                emit(Event::ControlAck { kind: "rescope" });
            }

            ControlMessage::Interrupt => {
                state.status = LoopStatus::Interrupted;
                emit(Event::ControlAck { kind: "interrupt" });
                return;
            }

            ControlMessage::SkipCurrentStep => {
                // 注入提示（agent 下一轮看到）
                state.messages.push(Message::User {
                    content: ContentPart::Text(
                        "[Manager 让你跳过当前步骤，直接进行下一步]".into()
                    ),
                });
                emit(Event::ControlAck { kind: "skip_step" });
            }

            ControlMessage::PinPanel(panel_id) => {
                pin_panel_in_vault(&panel_id);
                emit(Event::ControlAck { kind: "pin_panel" });
            }

            ControlMessage::UserResponse(text) => {
                // 这是 awaiting_user 状态下用户的回答
                state.messages.push(Message::User {
                    content: ContentPart::Text(text),
                });
                state.status = LoopStatus::Running;
                set_task_status(state.task_id, "in_progress");
                emit(Event::ControlAck { kind: "user_response_received" });
            }
        }
    }
}
```

Control message 由 gateway 的 control endpoint 接收（HTTP POST /sessions/:id/control 或 WebSocket frame），写入 vault.events 并 push 到对应 AgentLoop 实例的 control_inbox（tokio mpsc channel）。

## 10. First-turn Routing & Task Extraction

> 前端通过 `POST /sessions/:id/messages` 提交用户输入（详见 [`api.md`](api.md) §4.3）。
> Gateway 接到 message 后**不直接创建 task**——而是经过一层 routing 决定该开 task 还是闲聊回复。
> Task 概念在前端隐式（[interaction-model.md §3.2](../interaction-model.md)）就是靠这层 routing 实现的。

### 10.1 Routing 决策树

```
POST /sessions/:id/messages 进 gateway
        │
        ├─ body.task_id 显式给定？
        │     ├─ Yes → 注入该 task 的 in-progress AgentLoop control_inbox
        │     │         （详见 §10.4 in-thread routing）
        │     └─ No  → 看 session 是否有 in_progress task
        │               ├─ 有 → 默认按 task_id 给定处理（attach 到当前 task）
        │               └─ 无 → 启动 extraction LLM call（§10.2）
        │
        └─ extraction 输出三态：
              ├─ new_task    → 创建 task（status=in_progress, source='user'）+ 启动 AgentLoop
              ├─ chat_reply  → 直接 chat 文本回复，不开 task
              └─ ambiguous   → 推一条 clarification message，不开 task；
                                 等用户下一条 message 重走 routing
```

### 10.2 Extraction Prompt

启动一次轻量 LLM call（小 max_tokens、复用主 system prompt cache 的前缀部分）。System prompt：

```
[Persona]
你是 L.E.E.K main agent 的 routing layer。你只做一件事：
决定用户这条消息要起任务还是闲聊。不要执行实际研究——只做意图分类。

[Context]
- Session 历史最近 N 条 message（默认 N=5）
- Team Charter（用户的偏好与硬约束）
- Session 中最近 confirmed deliverables 的摘要

[User Message]
{user_input}

[Output Schema] — 严格 JSON
{
  "decision": "new_task" | "chat_reply" | "ambiguous",
  "reason": "...",
  "task_draft"?: {                    // decision=new_task 时必填
    "title": "...",
    "goal": "...",
    "constraints"?: { ... },
    "expected_deliverable":
        "decision_draft" | "research_brief" | "review"
      | "comparison" | "morning_brief" | "free_form",
    "context_refs"?: [...]
  },
  "chat_reply_text"?: "...",          // decision=chat_reply 时必填
  "clarification_question"?: "..."    // decision=ambiguous 时必填
}

[Decision Rules]
- new_task：用户表达了具体研究目标 / 决策需求 / 复盘需求
  · "NVDA 现在能加仓吗" / "复盘上周 META 决策" / "看看 BABA 最近"
- chat_reply：闲聊 / 问候 / 元问题
  · "你好" / "今天日期"
  · "你刚才做到哪一步了"——task 元状态查询走这里，
    因为 TaskBar 已经显示进度，agent 只需要补一句话
- ambiguous：意图不明，需要 1 个澄清问题
  · "看看我的持仓" → 复盘 / 加仓建议 / 风险评估?

[expected_deliverable 推断启发式]
- 含 "买 / 卖 / 加仓 / 减仓 / 止损"   → decision_draft
- 含 "复盘 / 看看上次 / 当时怎么"     → review
- 含 "对比 / vs / 哪个 / 比较"         → comparison
- 含 "了解 / 调研 / 看看"              → research_brief
- 含 "晨报 / 早报 / 今天关注"          → morning_brief
- 否则                                  → free_form
```

成本估算：~500-2000 input tokens（命中 system prompt cache 后实际计费 ~200 tokens） + 200-500 output tokens。

### 10.3 三态 decision 后的动作

| decision | Gateway 动作 |
|--|--|
| `new_task` | 1. 用 task_draft 创建 task（status=`in_progress`, source=`user`）<br>2. 写入 `vault.tasks` + `task_assignments`<br>3. 启动 AgentLoop（绕过 queued state——reactive 路径直接执行）<br>4. emit `task_created` + `task_started` events<br>5. AgentLoop 第一轮 LLM call 用 main system prompt（§4.1） |
| `chat_reply` | 1. 把 `chat_reply_text` 作为 agent message 写入 vault.messages（content_json type=text）<br>2. emit `agent_message_start` / `agent_message_delta` / `agent_message_end`（按字符 chunk 流出）<br>3. 不创建 task |
| `ambiguous` | 1. 把 `clarification_question` 作为 agent message 写入<br>2. emit 同上 + `clarification_requested` event<br>3. 不创建 task；下次 user message 进来重走 routing |

### 10.4 In-thread message routing

当 message 已有 task_id（显式或推断 = session 中存在 in_progress task）时，gateway 把它注入对应 AgentLoop 的 `control_inbox`：

```rust
match loop_status {
    LoopStatus::AwaitingUser => ControlMessage::UserResponse(text),
    _                        => ControlMessage::AppendConstraint(text),
}
```

main agent 在下一个 yield point 处理（详见 §9）。在 `build_chat_request` 中追加一段提示让 agent 自己判断这条 in-thread message 的性质：

```
[In-thread Message Handling]
用户在当前 task thread 内追加: "{text}"
请判断是哪一种：
  A. 当前 task 的追加约束（"也考虑 BABA"）→ 直接纳入 context，下一轮工作中体现
  B. 要 fork 新 task（"另开一个看 GOOGL"）→ 调 task.fork 工具
  C. 闲聊 / 元问题（"做到哪一步了"）→ 简短文本回复，继续工作
默认按 A 处理，仅在明确换方向时按 B。
```

这个判断是 in-loop 的，不再走 routing layer——routing layer 只在 "session 无 in_progress task" 时启动。

### 10.5 与 Proactive task 的关系

cron / agent_proposed 创建的 task 走 `POST /api/v1/tasks`（[`api.md`](api.md) §4.2）——
**不经过** routing layer，直接进 vault 的 `draft` / `queued` state。

- `draft` task 在前端显示为 TaskBar 顶部 banner（"🔔 1 个待复盘任务"）等用户接受
- 用户点 [立即开始] → `POST /tasks/:id/submit` → status=`queued` → scheduler pickup → AgentLoop in_progress

implicit extraction（reactive）+ POST /tasks（proactive）这两条路径互补，覆盖所有 task 来源
（详见 [interaction-model.md §3.2](../interaction-model.md)）。

### 10.6 Routing layer 的实施位置

routing layer 是 gateway 的一部分，不是独立 service：

```rust
// crate::gateway::router

async fn handle_user_message(session_id, body) -> Response {
    let route = decide_route(session_id, body.task_id).await?;
    match route {
        Route::ExistingTask(tid) => inject_to_loop(tid, body.content).await?,
        Route::NewExtraction => {
            let extraction = run_extraction_llm(session_id, body.content).await?;
            apply_extraction(session_id, extraction).await?
        }
    }
}
```

extraction LLM call 复用 `LlmRegistry`——同 main loop 一个 provider chain；fallback 同样适用。
extraction call 的 usage log invoker 字段标 `system:routing`，便于审计。

## 11. Task Lifecycle 与 Loop 的对接

```
Task source              Task lifecycle           AgentLoop 状态           Vault 写入
─────────────────────────────────────────────────────────────────────────────────────
implicit (§10.3)         (跳过 queued)
  POST /messages →                                init_state
  routing → new_task                              状态=Running            tasks.status='in_progress'
                                                                          tasks.source='user'
                                                                          tasks.started_at=now

proactive
  POST /tasks            queued                                            tasks.status='queued'
  cron / agent_proposed     ↓ user submit / scheduler 调度
                         init_state
                         状态=Running            tasks.status='in_progress'
                                                  tasks.started_at=now
                         loop iteration 1..N
                                                  events.* (per iteration)
                                                  tool_call_runs.*
                                                  subagent_runs.*
                                                  reasoning_dag_traces (增量)
                                                  artifacts.* (panel)
                         
   ↓ agent 调 clarify.ask_user
awaiting_user            状态=AwaitingUser        tasks.status='awaiting_user'
                         loop 退出
   ↓ 用户答复
                         schedule 新一轮          
                         control: UserResponse    
                         状态=Running            tasks.status='in_progress'
   ↓ 继续
                         agent 调 decision.draft  deliverables (status=draft → ready)
                         状态=Done                tasks.status='delivered'
                                                  tasks.delivered_at=now
   ↓ 用户 review
delivered → confirmed                            decisions.* (派生写入)
                                                  tasks.status='confirmed'
                                                  tasks.closed_at=now
```

## 12. 错误处理与恢复

### 12.1 Agent loop 崩溃

如果 loop 因为 panic / 不可恢复错误崩了：
- 写 `tasks.status='failed'` + `status_reason`
- emit `Event::TaskFailed { reason }`
- 不自动重试（除非用户主动 retry）

### 12.2 LLM provider 错误

- Auth invalid / rate limited / quota exceeded → registry 自动 fallback 到下一 provider（详见 llm-provider.md §3）
- 全部 provider 失败 → loop 状态变 Failed + emit Error 事件
- Routing layer extraction call 同样走 fallback；全部失败 → 直接当 chat_reply 处理（保底回复"抱歉服务暂不可用"），不创建 task

### 12.3 Tool 错误

- 单次 tool call 失败 → ToolError 作为 ToolResult 内容回填给 LLM，agent 决定怎么处理（重试 / 换参数 / 放弃）
- 多次同一 tool 错误 → agent 应当自己判断放弃，但 loop 不强制阻断

### 12.4 中断恢复

- Task 状态 `cancelled` 后所有 state 持久化保留
- 用户可以从 cancelled state 恢复（task 状态 → queued，loop 重新启动会读 vault.events 重建 messages）
- P1 简化：cancelled 后允许 fork 新 task（带 context），但**不允许真正"continue from where left off"**——因为 LLM 状态难以完美恢复
- 真正的 continue 是 P2 议题

## 13. 性能与并发

### 13.1 Loop 并发模型

- 每个 active task 一个 `AgentLoop` 实例（tokio task）
- 一个用户最多同时跑 5 个 active task（防止 quota 耗尽）；超出排队
- 全 gateway 最多 50 个 active loop（防止单机过载）；超出 task 进 queued 等位

### 13.2 Yield Point

每次 LLM 调用之间都是 yield point；每个 tool dispatch 之间也是 yield point。Control message 在 yield point 才生效——意味着用户中断 agent 后**最多等一个 LLM call 时间**（典型 1-3s）才看到 ack。

P2 增强：流式中也能中断（让 LLM provider 主动 close 流），但实施成本高，P1 不做。

### 13.3 Token 预算追踪

每次 LLM 调用结束写 `llm_usage_log` + 累计 `task.tokens_used`。task 超 budget → emit warning 但不强制停（让 agent 自己判断）。

Routing layer 的 extraction call 也写 usage log，invoker 字段标 `system:routing`（不计入任何 task 的 tokens_used，便于审计 routing 总成本）。

## 14. 实施 checklist

- [ ] **Routing layer**：`handle_user_message` + 决策树 + extraction LLM call
- [ ] **Extraction prompt template**（§10.2）+ JSON schema 校验
- [ ] **Three-way decision dispatcher**：new_task / chat_reply / ambiguous → vault 写入 + event 发射
- [ ] **In-thread message handling** prompt 注入（§10.4）
- [ ] `AgentLoop` struct + `AgentLoopState`
- [ ] Loop 主循环（含 control message 处理 / outcome 解析）
- [ ] `build_chat_request` + system prompt template
- [ ] Context 裁剪（P1 简化版）
- [ ] `consume_stream` 流式事件解析
- [ ] `dispatch_tools` 并行调用
- [ ] `spawn_subagent` 子循环
- [ ] Reasoning DAG 自动节点生成
- [ ] Control message inbox（mpsc channel + control endpoint）
- [ ] Task lifecycle 状态机（implicit 直进 in_progress / proactive 走 queued / awaiting_user / delivered / ...）
- [ ] 错误处理 + provider fallback（loop + routing 双路径）
- [ ] Loop 实例池 + 并发上限
- [ ] 单元测试：每个 outcome 类型 + control message 处理 + extraction 三态
- [ ] 集成测试：完整 task 端到端（mock LLM provider）
- [ ] e2e 测试：真实 LLM + 真实 tools，跑一个 decision_draft 类型 task
