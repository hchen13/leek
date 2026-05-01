# ADR 0004 — P1 不做 ACP adapter

- **状态**：Accepted
- **日期**：2026-05-01
- **决策者**：项目所有者
- **相关 ADR**：[0005](0005-self-implemented-harness.md)（自实现 harness）、[0007](0007-event-protocol-and-transports.md)（MCP HTTP 已覆盖外部 agent 接入）

## Context

ACP（Agent Communication Protocol）在早期设计讨论中被列为 P1 候选 adapter，灵感来自 hermes-agent 的 `acp_adapter/` 目录。当时的假设：

- ACP 是某种 agent-to-agent 通信协议（hermes-agent 里和 MCP 并存）
- 把 ACP 作为 P1 adapter，外部 agent 可以通过 ACP 把任务交给 leek

实际深入分析时，发现两个问题：

1. **协议语义不清**：项目所有者从未接触过 ACP，handoff §3 也明写 "待用户深入研读源码"。在没有 spec 实证的情况下，把它列为 P1 adapter 是对未知协议的承诺。

2. **方向性混淆**：DeerFlow 的 `invoke_acp_agent` 是 leek-as-client 调外部 ACP agent（出站）；handoff 假设的"ACP adapter"是 leek-as-server 让外部 agent 接入（入站）。两者完全不同，先前的设计文字没有区分。

3. **场景被现有 adapter 覆盖**：

   | 外部 agent 用 leek 的场景 | 谁是 thinker | 现有方案 |
   |--|--|--|
   | 外部 agent 把 leek 的工具当 MCP 工具调用 | 外部 agent | **MCP HTTP** ✓ |
   | 外部 agent 委托一个长任务给 leek，自己做调度方 | leek 自己（自带 harness） | **自实现 harness + WebSocket 订阅进度** ✓ |
   | 两个对等 agent 双向调用 | 双方各自 | 不在 P1 范围 |

   前两类已经被 MCP HTTP（[0007](0007-event-protocol-and-transports.md)）和自实现 harness（[0005](0005-self-implemented-harness.md)）完整覆盖。

## Decision

**P1 不做 ACP adapter。** 不在 gateway 暴露 ACP 端点，不实现 ACP 协议。

外部 agent 接入 leek 的两条主要路径：

- **接 leek 的工具能力（leek 不思考）**：用 MCP HTTP（streamable-http transport），leek 暴露 corpus 检索 / 行情 / 技术指标等工具
- **委托长任务给 leek（leek 自己思考）**：HTTP POST 创建 session + WebSocket 订阅事件流，事件协议见 [ADR-0007](decisions/0007-event-protocol-and-transports.md)

## Consequences

### 简化 P1 实施范围

少一个协议要 implement，少一份 spec 要 maintain。Gateway 只需要 HTTP + SSE + WebSocket + MCP HTTP，这套已经足够覆盖所有 P1 接入场景。

### 不堵未来路径

未来如果 ACP 协议成为事实标准、或某个生态系统（如 hermes-agent / Cloudflare AgentKit / Anthropic 某个新协议）强制需要 ACP，**架构允许后加**：
- ACP adapter 与现有 SSE / WebSocket adapter 是同级 transport
- 内部 EventBus 是统一的，只要写一个 ACP transport adapter 接通 EventBus 即可
- 这个改动是增量的，不会改 agent core 或 vault 任何代码

### 项目所有者要心里有数

这个决策建立在两个假设上：

1. MCP HTTP 已经事实成为 LLM 工具协议的主流（Anthropic、Cursor、Claude Code、Cline、Continue、Open WebUI 都支持）
2. 外部 agent 委托长任务的场景，HTTP POST + WebSocket 已经是事实标准模式（OpenAI Assistants API、所有主流 agent harness 都长这样）

如果未来出现一个不支持 MCP 的强势 agent 生态，且我们想接它，可能需要重新考虑。当前判断该风险低。

## Alternatives Considered

### 现在做 ACP adapter 占位（被否）
- 在不理解协议语义的情况下实现适配层，结果大概率是 toy 实现，将来真要用还得重写
- 占用 P1 工作量但不解决 P1 用户场景

### 完全不留 ACP 入口（采纳）
- 当前选择
- 真要做时直接加新 transport，无前置阻塞

### 延迟到读完 hermes-agent 源码再决定（被否）
- 读 hermes-agent 不是 P1 阻塞项
- 即便 hermes 用 ACP，我们的接入需求由 MCP HTTP 和 WebSocket 已经覆盖；ACP 在 leek 里没有独立必要性

## 重新评估的触发条件

- **触发 1**：出现 P1 用户必须接入但只支持 ACP 的 agent 生态
- **触发 2**：ACP 协议出现公认的官方 spec（如成为 W3C / IETF 标准），且生态广度大于 MCP
- **触发 3**：项目所有者在使用 hermes-agent 或类似系统时发现 ACP 提供 MCP / WebSocket 都不能替代的能力

任一触发出现，重开 ADR 评估是否新增 ACP adapter。
