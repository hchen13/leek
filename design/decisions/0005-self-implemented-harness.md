# ADR 0005 — 自实现 harness + LLM provider 抽象（OAuth + API key）

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0001](0001-rust-gateway.md)（语言决定 SDK 与 vendor 选择）、[0004](0004-no-acp.md)（不依赖外部 harness 的协议接入）

## Context

L.E.E.K 的 agent 思考能力来源有三个候选方向：

1. **Vendor 现成的 agent harness**：把 leek 当某个 harness（Claude Code / Codex CLI / hermes-agent）的前端 / 适配层，思考交给 vendor 处理，leek 只负责 corpus / vault / web UI / 工具
2. **基于 LangChain / LangGraph 等 agent 框架**：用 Python 写应用层，把 loop / scratchpad / tool calling 全交给框架
3. **自实现 harness**：leek 自己持有 agent loop、scratchpad、thinking、tool dispatch、conversation context

每个方向有不同含义：

| 方向 | 思考归属 | 灵活度 | P1 工作量 | 风险 |
|--|--|--|--|--|
| Vendor harness | vendor LLM | 受限于 vendor protocol | 极低 | vendor 政策 / 接口变化 / 不可深度定制 |
| LangChain / LangGraph | Python 框架内 | 中（受框架抽象约束） | 低 | 框架升级 churn / 与 Rust gateway 不兼容 |
| 自实现 | leek 自己 | 完全可控 | 高 | 需要自己处理 LLM provider / 流式 / tool use 协议演化 |

项目所有者明确选择方向 3 + 提出关键约束：**支持 OAuth（优先 Codex OAuth）和 API key 两种认证方式**。

> "我们自己实现 harness。支持 API-KEY 配置和 codex oauth。优先 oauth 吧因为我现在就有 codex 订阅，测试起来最方便"

这一选择把 leek 的定位从"另一个 agent harness 的 frontend"提升到"自带思考能力的投研系统"，与 self-improving loop（"corpus 增长率 = 系统改善率"）的护城河愿景一致。

## Decision

**leek 自实现 agent harness。** 不 vendor Claude Code / Codex / LangChain / 任何现成 harness。

### Harness 核心组成

```rust
pub struct AgentLoop {
    session: Session,
    provider: Box<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    event_bus: Arc<EventBus>,
    scratchpad: Scratchpad,
}

impl AgentLoop {
    pub async fn run_turn(&mut self, user_input: Message) -> Result<Reply> {
        // 1. 构建 context（system prompt + corpus refs + vault state + mandate + history）
        // 2. LLM 调用（流式接收 thinking / tool_calls / text deltas）
        // 3. 解析 tool_calls，并行 dispatch 到 ToolRegistry
        // 4. tool 结果回填，迭代或终止
        // 5. 全程向 EventBus 推事件
    }
}
```

### LLM Provider 抽象

```rust
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;  // thinking, tool_use, vision, max_context, ...
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        opts: ChatOptions,
    ) -> Result<BoxStream<LlmEvent>>;
}

pub enum Auth {
    ApiKey(String),
    OAuth(OAuthBundle),  // access_token + refresh_token + expires_at
}

pub enum LlmEvent {
    ThinkingDelta(String),
    TextDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallArgsDelta { id: String, delta: String },
    ToolCallEnd { id: String },
    Usage { input_tokens: u64, output_tokens: u64, cache_hit: u64 },
    Done,
    Error(LlmError),
}
```

### P1 实现的 provider

| Provider 名 | Auth | Backend Endpoint | 备注 |
|--|--|--|--|
| `codex_oauth` | OAuth | ChatGPT 订阅 backend（OpenAI Responses API） | 开发期默认；用户自带订阅 quota |
| `anthropic_api_key` | API key | `api.anthropic.com` | 兜底；Claude 系列模型 |
| `openai_api_key` | API key | `api.openai.com` | 兜底；GPT 系列 |

### Codex OAuth 流程

**手抄进 leek**，不 vendor `openai/codex` 仓库（codex 还在快速迭代，作为 vendor 依赖不稳）。

实现要点：
- OAuth device flow（用户在浏览器授权，leek 长轮询拿 token）
- token 持久化在 `~/.leek/auth.toml`（权限 0600）
- refresh_token 自动刷新逻辑
- 调用 endpoint 时模拟 codex client 的必要 headers / UA / 协议形态
- 失败时降级到 API key path

### 两条腿走路（防 OpenAI 政策风险）

**OAuth 路径不能是单点依赖**：

- Codex OAuth 走的 ChatGPT backend endpoint 是 OpenAI 半官方接口，第三方 client 复用 OAuth token 走同一 endpoint，OpenAI 政策上**随时可能限制**（指纹检查、UA 校验、签名机制）
- 因此 P1 必须**两条腿走路**：API key 路径同样跑通，OAuth 断了就退回 API key
- 任何使用 OAuth 的地方都必须有 API key 替代路径，**不允许出现"功能 X 仅 OAuth 可用"**

## Consequences

### 完全可控的 agent loop

- 上下文裁剪策略可以为投研场景定制（如 "保留近 3 轮的 tool 结果 + 全部 user message + 用户标记为 important 的 panel"）
- Thinking 模式接入可以为不同 provider 写不同适配（Anthropic 的 extended thinking vs OpenAI 的 reasoning_effort）
- Tool use 协议层的差异在 provider 层吸收（Anthropic tool_use block vs OpenAI tool_calls），上层 ToolRegistry 看到的是统一接口

### LLM 协议演化要自己跟

- 新 feature 出现时（如 vision 新形态、新 reasoning 模式、新 cache 机制）要自己写 HTTP 协议封装
- 每个 provider 的 SSE 事件格式差异要在 provider 层吸收
- **代价**：每次 provider 大改要花时间适配
- **收益**：不被任何 SDK 升级节奏拖住

### Codex OAuth 是开发期 testing 之友

- 用户已有 ChatGPT 订阅，开发时可以零额外成本快速 iterate
- API key 路径用于 production / CI / 给其他用户（无 ChatGPT 订阅）

### 不能复用其他 harness 的代码

- DeerFlow 的 LangGraph 派生不能直接拿来
- hermes-agent 的 agent core 不能直接拿来
- 全部要在 Rust 里重写一遍

接受这个代价——换来的是 Rust 单二进制 + 完全可控的 loop + 适配 leek 投研场景的定制能力。

## Alternatives Considered

### Vendor Claude Code 当 thinker（被否）
- Claude Code 是个 CLI agent，不暴露稳定的"agent loop"程序接口
- 把 leek 做成 Claude Code 的 frontend = leek 永远是 Claude Code 的下游
- 投研 corpus + vault + 决策追踪的核心闭环没法在 Claude Code 内部实现，最终还是要自己跑
- 用户主动放弃这条路：明确说"自己实现 harness"

### Vendor Codex CLI 当 thinker（被否）
- 同上理由
- Codex CLI 也在快速迭代，依赖它意味着持续 churn

### LangChain / LangGraph（被否）
- 主要是 Python 框架，与 Rust gateway 选型不兼容
- 即使有 langchain-rust，社区不成熟，且 LangGraph 这一层的核心价值（visual workflow）我们的投研场景用不上
- agent loop 本身不复杂（核心 < 500 行 Rust），自实现成本可控

### Anthropic / OpenAI 官方 SDK（部分否）
- Rust 官方 / 社区 SDK 都不够 1A 级别，且我们要走 OAuth + API key 两种 auth
- 直接 HTTP 反而更稳：JSON 字段级控制、不被 SDK 升级节奏拖住
- ADR-0001 已确认 Rust 走 HTTP 直连

## 验证标准

- 完整一个 turn（user input → context build → LLM stream → tool calls → result → final reply）的 harness 核心代码 < 1000 行 Rust
- 切换 provider 通过修改配置完成，agent loop 代码不动
- Codex OAuth 与 API key 路径都能完成同一个测试 session（长 ~10 turn 含 tool calls）
- OAuth 断（如手动撤销 token）时自动降级到 API key 不中断 session
