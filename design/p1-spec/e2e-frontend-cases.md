# Running the 10 frontend E2E cases

Slice scope: validate that the harness changes (plan resolution, budget
finalization, corpus_read, follow-up continuity, validation UX, model effort
coercion) hold up against the user-facing test suite. These cases are meant
to be run **from the browser** at `http://127.0.0.1:5173/`, not from the API.

Each case must produce a visible session, a chat backbone, and the right
mix of plan / tool / subagent / deliverable / error cards. No spinner-stuck
states; no silent task_failed; no decision_draft on a request that didn't
ask for a recordable action.

## Local prerequisites

```sh
# 1. Start the gateway (port 8964 is the default).
cargo run -p leek-gateway --bin leek -- serve --port 8964

# 2. Start the frontend dev server (proxies to 8964 — see vite.config.ts).
cd frontend/web && npm run dev
# → opens http://127.0.0.1:5173/

# 3. Optional — clear the vault between runs to keep sessions isolated:
rm -f vault.db && cargo run -p leek-gateway --bin leek -- auth codex --import-from-codex-cli
```

For each case, open a **new session** from the SessionMenu (top-left rail
chevron → "新建会话") so the artifacts don't blur into prior runs. The
session list should always include every browser-created test session
without manual refresh.

## S1 — `用一句话解释能力圈是什么，不要做研究。`

Routing → `chat_reply` (single-line definition; no investigation).
- Expected UX: agent bubble streams a one-sentence answer.
- No plan card. No tool calls. No deliverable.
- `▸ thinking · Ns` flips to `✓ done · Ns`.

## S2 — `用 corpus 解释 margin of safety 和 owner earnings 的关系，给 3 点。`

Routing → `chat_reply` (definition / synthesis from corpus only).
- Expected: one or more `corpus_search` cards in the canvas; possibly a
  `corpus_read` card on the wiki hits. **No `web_fetch` / `web_search`
  cards** — the system prompt now disables web for "用 corpus" turns.
- Plan optional; if the agent makes one, every item ends `completed` with
  `done` resolution.
- Final answer: 3 points, fact / inference / speculation labels.

## S3 — `评估一下 NVDA 现在加仓；如果不能给动作，也要明确原因。`

Routing → `new_task` with `expected_deliverable=research_brief` (NOT
decision_draft — the user is asking for an evaluation, not a recordable
trade).
- Expected: plan card builds, tool calls run (corpus_search, web_search,
  market_quote, web_fetch, optionally corpus_read).
- If NVDA realtime data is available: substantive write-up at the end.
- If realtime data fails after retry: agent calls `update_plan` to close
  the affected items as `blocked` / `insufficient_evidence` with evidence,
  delivers a partial brief that names the gap and the conservative action
  boundary. **No silent task_failed.**
- If MAX_TOOL_TURNS or plan_guard exhausted: budget_finalization banner
  ("BUDGET CHECKPOINT · 工具调用预算用完") appears above the agent
  message; the message itself is the checkpoint answer (5 sections).

## S4 — `快速提交一个 NVDA 加仓 decision draft，但不要写 risks。`

Routing → `new_task` with `expected_deliverable=decision_draft` (explicit
"提交 decision draft" → recordable action).
- Expected: agent calls `record_investment_action` once with `risks=[]`.
  The tool returns a structured validation error
  ("validation: 'risks' must contain at least one named, concrete risk…
  Do NOT retry with fabricated platitudes…").
- Tool card on canvas shows `✗ record_investment_action failed` with the
  validation message; the agent message must explain to the user that the
  draft cannot be submitted and (per the agent loop hint) NOT silently
  retry with fabricated risks.
- If the agent ends without producing the draft, the deliverable_ready
  emitted is `research_brief`-shaped final text, not a decision.

## S5 — `请直接调用 delegate_research，让 risk_manager 用三点列出 NVDA 加仓风险，然后展示结果。`

Routing → `new_task` with `expected_deliverable=research_brief` (a risk
list is research output, not a recordable trade — the prompt explicitly
calls delegate_research, not record_investment_action).
- Expected: subagent card on canvas (`subagent[risk_manager]
  in_progress` → `completed`); critic / main agent then produces the
  final write-up with the three risks.
- Critic uses `Low` reasoning_effort (no longer `Minimal`), so the call
  goes through gpt-5.5 without the upstream "Unsupported value 'minimal'"
  rejection that used to break this case.
- Final deliverable kind is `research_brief`. **No DecisionDraftCard on
  this case** — the UI's `decision_draft_ready` event does not fire.

## M1 — challenge prior valuation

Sequence:

1. `评估 NVDA 现在是否值得加仓。` (new_task / research_brief)
2. After the analysis lands: challenge the price / valuation in the same
   session (`你刚才的估值假设里 WACC 取了多少？我觉得偏低，重新算一下`).
3. After response: `给我一个最终的 action boundary 和 invalidation conditions`.

Expected:
- Step 1 creates a task; plan + tools + deliverable.
- Step 2 routes to `chat_reply` (continuation); the agent stays in the
  same session and refines, no new task created.
- Step 3 also `chat_reply`; agent produces an explicit action boundary
  and invalidation conditions inline (does not create a fresh
  decision_draft — user did not ask for that).

## M2 — comparison + portfolio guardrail

Sequence:

1. `比较 AMD 和 NVDA，谁更适合未来 12 个月新增仓位？` (new_task /
   comparison)
2. After response: challenge — `如果 ASIC / Broadcom / hyperscaler
   self-designed chips 抢走 30% 训练负载呢？`
3. After response: `如果我已经同时持有 AMD 和 NVDA，应该怎么处理这个组合？`

Expected:
- Step 1 → comparison deliverable with side-by-side scoring.
- Step 2 → chat_reply continuation; agent revises both bull/bear cases.
- Step 3 → chat_reply; agent gives a portfolio action (trim / hold / add
  weights) without creating a new decision_draft.

## M3 — bubble framing + portfolio impact

Sequence:

1. `AI capex 会不会是一个泡沫？对半导体持仓怎么影响？` (new_task /
   research_brief)
2. After response: `给我反方证据 + leading indicators，要可观测的`.
3. After response: `把它翻译成 portfolio guardrails`.

Expected:
- Step 1: plan with corpus / macro / industry items; tool sequence may
  include `corpus_search` → `corpus_read` (full Dalio / cycles wikis),
  not `web_fetch` on those ids.
- Steps 2 / 3: chat_reply continuation. The agent produces concrete
  observable indicators and a guardrail set in the same task thread.

## M4 — mandate-bounded sizing

Sequence:

1. `假设我的单票上限 5%，NVDA 已经 4.5%，现在还能加吗？` (new_task /
   research_brief — mandate-bounded judgement, not a recordable trade
   unless the user explicitly asks).
2. Follow-up: `什么 portfolio 信息缺了？`
3. Follow-up: `给我一个 no-trade / trim / add 决策树`.

Expected:
- Step 1: agent uses `ask_user_question` if portfolio context is missing,
  OR proceeds with mandate-anchored judgement.
- Step 2: chat_reply continuation; lists the specific missing portfolio
  inputs.
- Step 3: chat_reply continuation; outputs a decision tree, not a
  decision_draft.

## M5 — post-mortem with reusable checklist

Sequence:

1. `假设我去年因为估值高卖飞了 NVDA，帮我复盘这是不是错误。` (new_task
   / review).
2. Follow-up: `这是 outcome bias 还是 process error？`
3. Follow-up: `帮我把 lessons 沉淀成一份 checklist 更新`.

Expected:
- Step 1 review deliverable with Bear/Base/Bull retrospective lenses.
- Step 2 chat_reply; agent distinguishes process from outcome.
- Step 3 chat_reply; agent produces / proposes a checklist update; should
  call `record_research_note` (not `record_investment_action`).

## What to watch in DevTools / console

- SSE event types you should see (Cmd+E opens the events drawer):
  `user_message`, `agent_message_start`, `agent_message_delta`,
  `agent_narration`, `tool_call`, `web_search_call`, `plan_updated` (with
  `resolution` on completed items), `subagent_run`, `deliverable_ready`
  (with `budget_limited: true|false`), `decision_draft_ready` (S4 only,
  and only after a successful record_investment_action), `task_*`,
  `budget_finalization` (only when caps trip), `error` only on real
  failures.
- The plan widget renders a small `lk-plan-resolution` chip on every
  closed item: `done` / `satisfied_by_proxy` are green; `blocked` /
  `insufficient_evidence` are red; `deferred` / `superseded` are amber.
- A budget-limited deliverable is still `task_delivered` (not
  `task_failed`), and the agent bubble carries an inline
  `BUDGET CHECKPOINT` banner. The composer remains usable.

## Failure modes to flag back to the harness

- DecisionDraftCard appearing on S3 / S5 / M1-M5 → routing prompt
  miscategorized the request. Check `expected_deliverable` in the
  events timeline; should never be `decision_draft` for those cases
  unless the user explicitly asked.
- A `task_failed` card with no checkpoint banner on S3 / M2 / M3 / M4 →
  budget recovery did not engage; verify `MAX_TOOL_TURNS = 24`,
  `MAX_PLAN_GUARD_REWRITES = 3`, and that the `budget_finalization`
  event was emitted before the run ended.
- `web_fetch` card with a wikilink id in `arguments` → the corpus_read
  pointer in `web_fetch`'s description / harness prompt isn't taking;
  re-check `looks_like_corpus_id` is firing.
- Spinner that never resolves → the agent loop did not emit a terminal
  `agent_message_end`; check the SSE events drawer for the last event
  before the stall.
