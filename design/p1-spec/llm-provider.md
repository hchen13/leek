# P1 Spec — LLM Provider 抽象与 Codex OAuth

> Provider trait 完整定义，三个 P1 实现的细节，Codex OAuth device flow 完整流程，UI 配置形态描述（供 UX 设计师做"Settings / Provider 配置"page 用）。

依赖：[ADR-0001](../decisions/0001-rust-gateway.md)（HTTP 直连）、[ADR-0005](../decisions/0005-self-implemented-harness.md)（自实现 harness + 多 provider + 两条腿走路）。

## 1. 设计目标

1. **统一抽象**：上层（agent loop / tool dispatch）看到的 provider 接口一致，不知道底层是 Anthropic / OpenAI / Codex
2. **HTTP 直连，零 SDK**：所有 provider 走 `reqwest` + 自己手写的 JSON 协议封装。新 feature 改 JSON 字段就跟上，不被 SDK 升级节奏拖累
3. **Auth 抽象**：API key 和 OAuth 在 trait 层等价，调用方不感知
4. **流式事件统一**：所有 provider 的 SSE 事件流转换成统一的 `LlmEvent` 枚举
5. **降级链**：OAuth 失败时自动降级到同一 provider 的 API key（如果配置了）；同 provider 失败到下一 provider；全部失败明确报错给用户

## 2. Trait 定义

```rust
// crate::llm::provider

use async_trait::async_trait;
use futures::stream::BoxStream;

/// 统一的 LLM provider 抽象
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider 唯一标识（如 "codex_oauth", "anthropic_api_key"）
    fn name(&self) -> &str;
    
    /// 当前生效的模型（可能从 config 读取或动态选择）
    fn current_model(&self) -> &str;
    
    /// 能力声明
    fn capabilities(&self) -> Capabilities;
    
    /// 健康检查（不实际发送 LLM 请求；只验证 auth 是否还有效）
    async fn health_check(&self) -> Result<HealthStatus, ProviderError>;
    
    /// 流式 chat（核心方法）
    async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<LlmEvent, ProviderError>>, ProviderError>;
}

/// Provider 能力声明
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub thinking: bool,                // 是否支持 extended thinking / reasoning
    pub tool_use: bool,                // 是否支持 tool calling
    pub vision: bool,                  // 是否支持图片输入
    pub max_context_tokens: u32,       // 最大 context 长度
    pub max_output_tokens: u32,        // 单次最大输出
    pub supports_streaming: bool,      // 流式输出（P1 必为 true）
    pub supports_prompt_caching: bool, // prompt caching
    pub supports_parallel_tools: bool, // 并行 tool calling（一次输出多个 tool_use）
}

/// 健康状态
#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub auth_valid: bool,
    pub last_check_at: chrono::DateTime<chrono::Utc>,
    pub model_reachable: bool,
    pub note: Option<String>,
}

/// Chat 请求
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolSpec>,
    pub options: ChatOptions,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, Copy)]
pub enum MessageRole {
    User,
    Assistant,
    System,         // 仅在 OpenAI / Codex 中作为 message；Anthropic 用单独 system_prompt
    Tool,           // tool result
}

#[derive(Debug, Clone)]
pub enum ContentPart {
    Text(String),
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, output: String, is_error: bool },
    // P2 扩展：Image, Audio, ...
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,  // JSON Schema
}

#[derive(Debug, Clone, Default)]
pub struct ChatOptions {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub thinking: Option<ThinkingConfig>,
    pub stop_sequences: Vec<String>,
    pub cache_strategy: CacheStrategy,
}

#[derive(Debug, Clone)]
pub enum ThinkingConfig {
    /// Anthropic-style: 显式 enable + budget
    Anthropic { budget_tokens: u32 },
    /// OpenAI-style: reasoning effort
    OpenAI { effort: ReasoningEffort },
    /// Codex 走 OpenAI Responses API，与 OpenAI 同构
    Codex { effort: ReasoningEffort },
}

#[derive(Debug, Clone, Copy)]
pub enum ReasoningEffort { Minimal, Low, Medium, High }

#[derive(Debug, Clone, Default)]
pub enum CacheStrategy {
    #[default]
    Auto,                  // 让 provider 自己决定
    Aggressive,            // 尽量 cache（system prompt + tool definitions + 较老 message）
    None,                  // 不 cache
}

/// 流式事件（统一抽象）
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// 流开始（含元数据如 model name / id）
    Start { request_id: String, model: String },
    
    /// Thinking content（reasoning / extended thinking）
    ThinkingDelta { text: String },
    
    /// 普通文本输出
    TextDelta { text: String },
    
    /// Tool 调用开始
    ToolCallStart { id: String, name: String },
    
    /// Tool 调用参数流（增量 JSON 字符串）
    ToolCallArgsDelta { id: String, delta: String },
    
    /// Tool 调用结束
    ToolCallEnd { id: String },
    
    /// 完整一次输出结束（一个 message 完成）
    MessageEnd { stop_reason: StopReason },
    
    /// 资源使用统计
    Usage {
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
    },
    
    /// 流结束
    Done,
}

#[derive(Debug, Clone)]
pub enum StopReason {
    EndTurn,         // 自然结束
    MaxTokens,       // 达到 max_tokens
    StopSequence,    // 遇到 stop sequence
    ToolUse,         // 输出 tool_use 后等结果
    Error(String),
}

/// Provider 错误
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("auth invalid: {0}")]
    AuthInvalid(String),
    
    #[error("rate limited (retry after {retry_after_sec}s)")]
    RateLimited { retry_after_sec: u32 },
    
    #[error("quota exceeded")]
    QuotaExceeded,
    
    #[error("model not found: {0}")]
    ModelNotFound(String),
    
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("network: {0}")]
    Network(String),
    
    #[error("server error: {0}")]
    ServerError(String),
    
    #[error("response parse error: {0}")]
    ParseError(String),
    
    #[error("timeout")]
    Timeout,
    
    #[error("oauth refresh failed: {0}")]
    OAuthRefreshFailed(String),
}
```

## 3. Provider 注册与降级

```rust
// crate::llm::registry

pub struct LlmRegistry {
    providers: Vec<Arc<dyn LlmProvider>>,  // 按 priority 倒序
    fallback_chain: Vec<String>,           // ["codex_oauth", "anthropic_api_key", "openai_api_key"]
}

impl LlmRegistry {
    pub async fn chat(&self, request: ChatRequest) -> Result<...> {
        let mut last_error = None;
        for provider_name in &self.fallback_chain {
            if let Some(provider) = self.find(provider_name) {
                match provider.chat(request.clone()).await {
                    Ok(stream) => return Ok((provider.name().to_string(), stream)),
                    Err(e) if e.is_retriable_with_other_provider() => {
                        last_error = Some(e);
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Err(last_error.unwrap_or(ProviderError::AuthInvalid("no providers available".into())))
    }
}

impl ProviderError {
    fn is_retriable_with_other_provider(&self) -> bool {
        matches!(
            self,
            ProviderError::AuthInvalid(_)
                | ProviderError::RateLimited { .. }
                | ProviderError::QuotaExceeded
                | ProviderError::OAuthRefreshFailed(_)
        )
    }
}
```

`fallback_chain` 由用户在 settings 中配置（详见 §10 UI 配置形态）。

## 4. 三个 P1 Provider 实现

### 4.1 `anthropic_api_key`

**Endpoint**：`https://api.anthropic.com/v1/messages`（streaming via SSE）

**Headers**：
- `x-api-key: <key>`
- `anthropic-version: 2023-06-01`
- `content-type: application/json`
- `accept: text/event-stream`

**请求形态**（关键字段）：
```json
{
  "model": "claude-opus-4-7",
  "max_tokens": 8192,
  "system": "...",
  "messages": [...],
  "tools": [...],
  "stream": true,
  "thinking": { "type": "enabled", "budget_tokens": 5000 }
}
```

**SSE 事件解析**：将 Anthropic 的 `message_start / content_block_start / content_block_delta / content_block_stop / message_delta / message_stop` 事件转换成 `LlmEvent::*`。

**capabilities**：
- thinking: ✓（claude-opus-4-7 支持 extended thinking）
- tool_use: ✓
- vision: ✓
- max_context_tokens: 200000
- supports_prompt_caching: ✓（用 `cache_control: {"type": "ephemeral"}` 标记）

### 4.2 `openai_api_key`

**Endpoint**：`https://api.openai.com/v1/responses`（Responses API；streaming via SSE）

**Headers**：
- `Authorization: Bearer <key>`
- `content-type: application/json`
- `accept: text/event-stream`

**请求形态**：
```json
{
  "model": "gpt-5",
  "input": [...],          // messages 数组
  "tools": [...],
  "stream": true,
  "reasoning": { "effort": "medium" }
}
```

**capabilities**：
- thinking: ✓（reasoning effort）
- tool_use: ✓
- vision: ✓
- max_context_tokens: 视模型而定
- supports_prompt_caching: ✓

### 4.3 `codex_oauth`

**核心**：复用用户 ChatGPT 订阅的 quota，不消耗 platform.openai.com 配额。

**Endpoint**：`https://chatgpt.com/backend-api/codex/responses` —— **与 `openai_api_key` 不同**（后者是 `api.openai.com`）。OAuth token 走 ChatGPT subscription 后端，不是 platform API。

**Headers**：
- `Authorization: Bearer <oauth_access_token>`
- `Content-Type: application/json`

**Body**：OpenAI Responses API 标准 schema（与 `openai_api_key` 一致；详见 §5.4）。

**capabilities**：与 `openai_api_key` 同 backend；但模型清单受 ChatGPT subscription 限制（OAuth 路径暴露的是用户订阅 plan 包含的模型）。

## 5. Codex OAuth 详细流程

> **协议来源**：[hermes-agent](https://github.com/NousResearch/hermes-agent) `hermes_cli/auth.py` 已验证。
> hermes 与 codex CLI 共享 client_id `app_EMoamEEZ73f0CkXaXp7hrann`——OpenAI 给 codex 公开的 device flow client。

**关键约束**：codex CLI / VS Code 扩展 / leek **不能共享** `refresh_token`——任一方先 refresh 会让其他方失效（OAuth refresh token rotation）。leek 因此**自己存 token**，不动 `~/.codex/auth.json`（详见 §5.3）。

### 5.1 Device Authorization Flow（PKCE）

**Step 1: 请求 user code**

```http
POST https://auth.openai.com/api/accounts/deviceauth/usercode
Content-Type: application/json

{ "client_id": "app_EMoamEEZ73f0CkXaXp7hrann" }
```

返回：
```json
{
  "user_code": "ABCD-1234",
  "device_auth_id": "<opaque token>",
  "interval": 5
}
```

**Step 2: 引导用户授权**

CLI / UI 显示：

```
1. Open in browser:  https://auth.openai.com/codex/device
2. Enter code:       ABCD-1234
```

> verification URL 是 `auth.openai.com/codex/device`——专门的 codex device login 页，不是通用 ChatGPT auth。

**Step 3: 长轮询拿 authorization_code**

每 `interval` 秒（min 3、default 5）：

```http
POST https://auth.openai.com/api/accounts/deviceauth/token
Content-Type: application/json

{
  "device_auth_id": "<from step 1>",
  "user_code": "ABCD-1234"
}
```

| 响应 | 含义 |
|--|--|
| `200` + `{ authorization_code, code_verifier }` | 用户已授权（PKCE 配对） |
| `403` / `404` | 用户未完成，继续 poll |
| 其他 | error |

总 timeout：15 分钟。

**Step 4: 用 PKCE 换 token**

```http
POST https://auth.openai.com/oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=authorization_code
&code=<authorization_code from step 3>
&redirect_uri=https://auth.openai.com/deviceauth/callback
&client_id=app_EMoamEEZ73f0CkXaXp7hrann
&code_verifier=<code_verifier from step 3>
```

返回：
```json
{
  "access_token": "<JWT>",
  "refresh_token": "...",
  "expires_in": 3600,
  "token_type": "Bearer"
}
```

`access_token` 是 JWT，含 `exp` 字段——可本地 decode 判断是否 expiring 而无需调 server。

### 5.2 Token Refresh

每次发 LLM 请求前 check JWT 的 `exp` 字段。距过期 < 60s 时先 refresh：

```http
POST https://auth.openai.com/oauth/token
Content-Type: application/x-www-form-urlencoded

grant_type=refresh_token
&refresh_token=<current>
&client_id=app_EMoamEEZ73f0CkXaXp7hrann
```

响应：

```json
{
  "access_token": "<new JWT>",
  "refresh_token": "<可能是新的，可能没有>"
}
```

**关键**：`refresh_token` 可能 rotate——响应里有就用新的，没有沿用旧的。

#### Error 处理

| HTTP / error code | 含义 | leek 行为 |
|--|--|--|
| `200` | 成功 | 写新 token（含可能 rotate 的 refresh_token） |
| `400 invalid_grant` / `invalid_token` / `invalid_request` | refresh_token 失效 | 标 `oauth_invalid`，要求重登 |
| `400 refresh_token_reused` | 另一客户端（codex CLI / VS Code）已用同 token | 同上，提示"先 run `codex` 终端登录，然后重新 `leek auth codex`" |
| `401` / `403` | refresh_token 失效（保险栅） | 同 `invalid_grant` |
| `5xx` / network error | 暂时性 | 沿用旧 token + 下次请求 retry |

### 5.3 Token Storage

token 写入 `vault.llm_provider_configs`：

| 字段 | 值 |
|--|--|
| `provider_name` | `"codex_oauth"` |
| `auth_kind` | `"oauth"` |
| `oauth_access_token` | JWT 字符串 |
| `oauth_refresh_token` | refresh token |
| `oauth_expires_at` | 从 JWT `exp` 字段 decode |
| `oauth_scope` | 通常为 `null`（codex device flow 不显式给 scope） |

#### 不复用 `~/.codex/auth.json`

OAuth refresh token rotation 让 codex CLI / VS Code 扩展 / leek **不能**共享一个 refresh_token——任一方先 refresh 会 invalidate 其他方的 token，导致 `refresh_token_reused` 错误。

leek 走 hermes-agent 验证过的方案：自己跑一次 device flow，token 存进 vault，与 codex CLI 隔离。

#### Onboarding 快速路径：`--import-from-codex-cli`

`leek auth codex --import-from-codex-cli` 命令：

1. 读 `~/.codex/auth.json`，拷贝 access/refresh token 进 vault.llm_provider_configs
2. 警告用户："codex CLI 接下来若 refresh 会让 leek token 失效——使用 leek 期间避免在终端跑 `codex`"
3. 一次性，不持续同步

如果用户更愿意保持 codex CLI 可用，跑 `leek auth codex` 走完整 device flow（5min 完成），两方独立。

### 5.4 走 Responses API 的细节

```http
POST https://chatgpt.com/backend-api/codex/responses
Authorization: Bearer <access_token>
Content-Type: application/json

<OpenAI Responses API 标准 body>
```

观察（来自 hermes-agent 的运行经验）：

- **模型清单**：与 platform.openai.com 不完全一致；OAuth 路径主要给 ChatGPT subscription 包含的模型（如 `gpt-5`、`gpt-5-codex`）。具体清单运行时 inspect。
- **Rate limit 形态**：基于 ChatGPT plan 的 quota，不是 platform token-per-minute；具体规则不公开。
- **SSE 协议**：与 platform Responses API 一致（OpenAI 标准事件类型 `response.output_text.delta` / `response.completed` 等，详见 §7.2）。

### 5.5 政策风险与防御

OpenAI 政策上可以随时通过指纹 / UA 校验 / 签名机制限制非 codex 客户端复用 OAuth token。leek 的防御策略：

1. **复用 codex 公开 client_id**：`app_EMoamEEZ73f0CkXaXp7hrann`——OpenAI 知道有第三方客户端用这个（hermes / continue / aider 等都在用）
2. **快速降级**：OAuth path 失败立即切到 API key path（如果用户配置了）
3. **明确告知用户**：UI 上说明"Codex OAuth 是依赖 OpenAI 政策的 best-effort 路径，建议同时配 API key 兜底"
4. **不绑死功能**：API key 路径在功能上必须等价

实施基线：以"OpenAI 不主动封"为前提工作（hermes / continue / aider 等都在用，目前未见大规模封禁）；某次 OpenAI 政策变化导致 device flow 不可用，用户切到 API key 即可。

## 6. Prompt Caching 策略

P1 起步即启用 prompt caching（节省 token + 加速响应）。两套实现细节：

### 6.1 Anthropic

在 `system` 与 `tools` 数组的最后一项加 `cache_control: {"type": "ephemeral"}`：

```json
{
  "system": [
    { "type": "text", "text": "You are L.E.E.K..." },
    { "type": "text", "text": "...大段 system prompt...", "cache_control": {"type": "ephemeral"} }
  ],
  "tools": [
    { "name": "...", ..., "cache_control": {"type": "ephemeral"} }
  ]
}
```

可选地：把第一条 user message（含 portfolio + 当前 task context）也标 cache，这样长 thread 的 cache hit 率高。

### 6.2 OpenAI / Codex

OpenAI 的 prompt caching 是**自动**的（基于 prefix hash），调用方不需要做任何标记。但要注意：
- prompt 长度需 ≥ 1024 tokens 才会被缓存
- prefix 必须**完全一致**才命中（system + tools + 历史 messages 的 prefix 不变 → cache hit）

实现层面：把 system prompt + tools 定义放在 messages 列表最前面，随后是历史 messages。每次请求只 append 新 message，前面 prefix 保持不变。

## 7. 流式事件转换示例

### 7.1 Anthropic SSE → LlmEvent

```
event: message_start
data: { "message": { "id": "msg_123", "model": "claude-opus-4-7", "usage": {...} } }
   → LlmEvent::Start { request_id: "msg_123", model: "claude-opus-4-7" }
   → LlmEvent::Usage { ... } (if usage present)

event: content_block_start
data: { "index": 0, "content_block": { "type": "thinking" } }
   → (no event; track block index → "thinking" mapping)

event: content_block_delta
data: { "index": 0, "delta": { "type": "thinking_delta", "thinking": "Let me..." } }
   → LlmEvent::ThinkingDelta { text: "Let me..." }

event: content_block_start
data: { "index": 1, "content_block": { "type": "text" } }
   → (no event)

event: content_block_delta
data: { "index": 1, "delta": { "type": "text_delta", "text": "I'll check..." } }
   → LlmEvent::TextDelta { text: "I'll check..." }

event: content_block_start
data: { "index": 2, "content_block": { "type": "tool_use", "id": "toolu_456", "name": "quote" } }
   → LlmEvent::ToolCallStart { id: "toolu_456", name: "quote" }

event: content_block_delta
data: { "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"ticker\":" } }
   → LlmEvent::ToolCallArgsDelta { id: "toolu_456", delta: "{\"ticker\":" }

event: content_block_stop
data: { "index": 2 }
   → LlmEvent::ToolCallEnd { id: "toolu_456" }

event: message_delta
data: { "delta": { "stop_reason": "tool_use" }, "usage": { ... } }
   → LlmEvent::Usage { ... }
   → LlmEvent::MessageEnd { stop_reason: ToolUse }

event: message_stop
data: {}
   → LlmEvent::Done
```

### 7.2 OpenAI / Codex SSE → LlmEvent

OpenAI Responses API 的事件结构（注：本节描述基于 P1 实施期实测；事件名以官方为准）：

```
event: response.created
   → LlmEvent::Start

event: response.output_text.delta
data: { "delta": "I'll check..." }
   → LlmEvent::TextDelta { text: "I'll check..." }

event: response.reasoning.delta
   → LlmEvent::ThinkingDelta { text: ... }

event: response.tool_call.created
   → LlmEvent::ToolCallStart

event: response.tool_call.arguments.delta
   → LlmEvent::ToolCallArgsDelta

event: response.tool_call.completed
   → LlmEvent::ToolCallEnd

event: response.completed
   → LlmEvent::Usage + LlmEvent::MessageEnd + LlmEvent::Done
```

具体事件名 / 字段名以实测为准；每个 provider 写一个独立的 `EventConverter`。

## 8. 错误分类与重试

| 错误类别 | HTTP / 信号 | 处理 |
|--|--|--|
| auth invalid | 401 | OAuth → 尝试 refresh 一次；失败 → 标记 invalid + fallback；API key → 直接 fallback |
| rate limited | 429 | 读 `Retry-After` header；P1 简化：直接 fallback。P2：等待 + 重试 |
| quota exceeded | 429 with specific code | 立即 fallback |
| model not found | 400 | 立即报错（用户配置问题，不 fallback） |
| invalid request | 400 | 立即报错（程序 bug，不 fallback） |
| server error | 5xx | 重试一次；仍失败 → fallback |
| network timeout | timeout | 重试一次；仍失败 → fallback |
| parse error | — | 报错（实施期 bug） |

重试策略：指数退避 base 1s, max 3s, 最多 1 次。

## 9. 配置文件 / vault 持久化

P1 三种凭证存储位置：

| Provider | 存储 | 备注 |
|--|--|--|
| `anthropic_api_key` | `vault.llm_provider_configs.api_key_encrypted` | OS keyring 加密；fallback 文件权限 0600 |
| `openai_api_key` | 同上 | 同上 |
| `codex_oauth` | `vault.llm_provider_configs.oauth_*` | refresh_token 重要！丢了要重新走 device flow |

Settings UI 编辑 → 写 vault → registry 重新加载 provider 实例。

不在 `~/.leek/config.toml` 存敏感凭证（config.toml 用于非敏感配置如 corpus_path / 端口号）。

## 10. UI 配置形态（给 UX 设计师）

Provider Settings page 应该长这样（结构化描述供 UX 落稿）：

### 10.1 入口

- 顶层 Settings page → "LLM Providers" 章节
- 首次启动 onboarding 必经此页（gateway 没配 provider 不能开始任何 task）

### 10.2 主视图

```
┌────────────────────────────────────────────────────────────────┐
│ LLM Providers                                                  │
│                                                                │
│ Active fallback chain:                                         │
│   1. Codex OAuth         ✓ active     last used 2 min ago     │
│   2. Anthropic API Key   ✓ active     last used 1 day ago     │
│   3. OpenAI API Key      ⚠ disabled                           │
│                                                                │
│   [Edit chain order]                                           │
│                                                                │
│ ──────────────────────────────────────────────────────────     │
│                                                                │
│ Codex OAuth                              [⚙] [Test] [Disable]  │
│   Status: ✓ Authorized                                         │
│   Account: gradschool.hchen13@gmail.com (ChatGPT Plus)         │
│   Token expires in: 47 minutes (auto-refresh)                  │
│   Default model: gpt-5                                         │
│   [Re-authorize]                                               │
│                                                                │
│ Anthropic API Key                        [⚙] [Test] [Disable]  │
│   Status: ✓ Valid                                              │
│   Default model: claude-opus-4-7                               │
│   API key: sk-ant-***************************************(48)  │
│   Last error: —                                                │
│                                                                │
│ OpenAI API Key                           [⚙] [Test] [Enable]   │
│   Status: ⚠ Disabled                                           │
│   API key: not set                                             │
│   [Configure]                                                  │
│                                                                │
│ [+ Add Provider]                                               │
└────────────────────────────────────────────────────────────────┘
```

### 10.3 添加 Codex OAuth 流程

```
Step 1: 选择 Provider Type
  ┌──────────────────────────────────────────┐
  │ Add new provider                         │
  │                                          │
  │   ◉ Codex OAuth (recommended for dev)    │
  │   ○ Anthropic API Key                    │
  │   ○ OpenAI API Key                       │
  │                                          │
  │              [Cancel]  [Next]            │
  └──────────────────────────────────────────┘

Step 2: Device Flow 引导
  ┌──────────────────────────────────────────┐
  │ Authorize Codex OAuth                    │
  │                                          │
  │  1. 在浏览器打开:                         │
  │     https://chat.openai.com/auth/device  │
  │     [Open in browser]                    │
  │                                          │
  │  2. 输入 user code:                       │
  │      ┌──────────────────┐                │
  │      │   ABCD - 1234    │  [Copy]        │
  │      └──────────────────┘                │
  │                                          │
  │  3. 等待授权完成...                        │
  │     ⟳ Waiting for authorization (8 min)  │
  │                                          │
  │  ⚠ Codex OAuth 是 best-effort 路径       │
  │    建议同时配置 API key 作为兜底           │
  │                                          │
  │              [Cancel]                    │
  └──────────────────────────────────────────┘

Step 3: 成功 / 失败页
  ✓ Authorized successfully
  Account: <email>
  Plan: ChatGPT Plus
  Default model: [gpt-5 ▾]
  [Done]
```

### 10.4 添加 API Key 流程

```
┌──────────────────────────────────────────┐
│ Configure Anthropic API Key              │
│                                          │
│  API Key:                                │
│  [ sk-ant-************************ 👁 ]  │
│                                          │
│  Default model:  [ claude-opus-4-7 ▾ ]   │
│  Aliases (optional):                     │
│   reasoning →  [ claude-opus-4-7 ▾ ]     │
│   fast →       [ claude-haiku-4-5 ▾ ]    │
│                                          │
│  [Test connection]                       │
│  ✓ Valid · responded in 423ms            │
│                                          │
│              [Cancel]  [Save]            │
└──────────────────────────────────────────┘
```

### 10.5 Settings 页面给 UX 的关键约束

1. **API key 的输入框默认 mask**——`sk-ant-***` 显示，点击眼睛图标显示明文
2. **Test connection 是必经动作**——保存前必须 test 通过
3. **Fallback chain 必须可视化**——用户能拖动改顺序
4. **Status 要清晰**：`✓ active / ⚠ disabled / ✗ invalid` 三色一致
5. **每个 provider 都有 Disable 而非 Delete**——避免误删，先 disable
6. **OAuth 重新授权要单独按钮**：refresh_token 失效后专门走"re-authorize"按钮，不是退到 device flow 的"Add new"
7. **风险提示的文字要醒目但不恐吓**——Codex OAuth 的 best-effort 警告必须可见，但不要让用户觉得"一定会失效"

### 10.6 表单字段 schema 与 validation

#### API Key 表单（`POST /providers/:name/configure` with `auth_kind="api_key"`）

| Field | UI Control | Validation | Error message |
|--|--|--|--|
| `api_key` | password input + 👁 toggle | non-empty；prefix match per provider:<br>· anthropic: `sk-ant-`<br>· openai: `sk-`（含 `sk-proj-` 等变体） | "API key 格式不匹配 ${provider}" |
| `default_model` | dropdown | 必选；options 从 §4.1/4.2 静态 list（前端 hardcode + 升级时同步） | "请选择默认模型" |
| `model_aliases.reasoning` | dropdown | 可选 | — |
| `model_aliases.fast` | dropdown | 可选 | — |

提交前必须执行：

1. 客户端 validation 通过
2. `POST /providers/:name/test` 返回 `{ ok: true }`（如果失败：禁用 [Save] 按钮，显示 inline `error.message`）
3. Save 时 `POST /providers/:name/configure` 写入

#### OAuth Device Flow 前端 polling 协议

```
Step 1  用户点 [Add Codex OAuth]
   │
   ▼
Step 2  POST /providers/codex_oauth/configure { auth_kind: "oauth" }
        ← 拿到 { device_flow: { user_code, verification_uri_complete,
                                  polling_endpoint, expires_in, flow_id } }
   │
   ▼
Step 3  UI 显示 user_code + verification URL（点击 [Open in browser]）
        显示倒计时（expires_in - elapsed）
   │
   ▼
Step 4  前端 polling：每 5 秒 GET ${polling_endpoint}
        响应：{ status: "pending" | "authorized" | "expired" | "denied" }
   │
   ▼
Step 5  Polling 终止条件：
        · status = "authorized" → GET /providers/codex_oauth → 显示成功页
        · status = "expired"     → "授权超时（15 分钟未完成）" + [Try again]
        · status = "denied"      → "授权被拒绝" + [Try again]
        · 用户点 [Cancel]        → 停止 poll，DELETE /providers/codex_oauth
        · 浏览器关闭 / tab 切换  → 暂停 poll，回前台时立即 trigger 一次 poll 恢复
```

| 参数 | 值 |
|--|--|
| Polling 频率 | 5 秒 (与 OpenAI device endpoint rate limit 一致) |
| Total timeout | 15 分钟 (`device_flow.expires_in`) |
| 倒计时显示 | "⟳ Waiting for authorization (X min remaining)" |
| 手动检查按钮 | "已完成？检查" 立即触发一次 poll |

#### Error states UI

| Error | 触发 | UI 形态 |
|--|--|--|
| Network error | configure / test / poll 请求失败 | inline banner: "网络错误，请重试 · [重试]" |
| API key invalid | test 返回 `{ ok: false, error: "401 unauthorized" }` | inline error 在 `api_key` 输入框下方 |
| Provider quota exceeded | test 返回 quota error | warn banner: "Provider quota 已用完，可暂时切换到其他 fallback" |
| OAuth expired | poll 返回 `expired` | replace device flow 卡片为 "授权超时" + [Try again] CTA |
| OAuth concurrent flow | configure 时已有 pending flow | "已有进行中的授权流程，是否取消并重新开始？" + [Cancel current] [Continue old] |
| Save 时 server reject | `POST /configure` 返回 4xx | top banner with error message + 保留表单状态 |

## 11. 资源使用 / 配额 UI（关联 Settings 页面）

可视化用户的 LLM 使用情况（从 `llm_usage_log` 聚合）：

```
┌────────────────────────────────────────────────────────────────┐
│ Usage (this week)                                              │
│                                                                │
│ Codex OAuth                                                    │
│   24,500 input · 18,200 output tokens · 47 calls               │
│   ▁▂▃▅█▆▄▂  daily distribution                                 │
│                                                                │
│ Anthropic API Key                                              │
│   8,200 input · 5,100 output tokens · 12 calls                 │
│   estimated cost: $0.42                                        │
│                                                                │
│ Cache hit rate: 68% (saving ~$0.30/week)                       │
└────────────────────────────────────────────────────────────────┘
```

P1 简化：只显示总量；详细按天 / 按 task 分布是 P1.5。

## 12. 测试与验证

### 12.1 单元测试（每个 provider）
- request 构造正确（snapshot 测试 JSON body）
- SSE 事件流解析正确（fixture 文件 → 期望的 LlmEvent 序列）
- 错误码映射正确

### 12.2 集成测试
- 真实 API 调用（用 `LEEK_TEST_PROVIDER=anthropic_api_key` 等环境变量控制）
- OAuth device flow 端到端（人工触发，CI 跳过）
- Fallback chain 行为（mock 一个 always-fail provider 验证降级）

### 12.3 e2e 验收
- 完整一个 task 的 LLM 调用，三个 provider 都能跑通
- OAuth refresh 自动触发（手动把 expires_at 改到过去时间，下次调用应当自动 refresh）
- OAuth invalid 自动降级（手动 revoke token，下次调用应当切到 fallback）

## 13. 实施 checklist

- [ ] `LlmProvider` trait + 数据类型定义
- [ ] `AnthropicProvider` 实现（含 SSE 解析 + cache_control）
- [ ] `OpenAiApiKeyProvider` 实现（Responses API + reasoning effort）
- [ ] `CodexOAuthProvider` 实现（device flow + refresh + 与 OpenAI 同 endpoint 但 OAuth bearer）
- [ ] `LlmRegistry` + fallback chain
- [ ] Provider config 持久化（vault.llm_provider_configs）
- [ ] Settings API（HTTP endpoints for UI）
- [ ] llm_usage_log 写入（每次 chat 完成）
- [ ] 单元测试 + fixture
- [ ] 集成测试用 mocked SSE server
- [ ] OAuth 重新授权流程（UI + backend）
