#!/usr/bin/env node

/**
 * 用法：
 *   node tests/api_e2e_runner.mjs
 *   node tests/api_e2e_runner.mjs --case 07
 *   node tests/api_e2e_runner.mjs --base-url http://127.0.0.1:8964 --timeout-ms 900000
 *
 * 输出：
 *   tests/e2e-results/<timestamp>.json
 *   tests/e2e-results/<timestamp>.md
 */

import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const CASES_PATH = path.join(ROOT, "tests", "a_share_e2e_cases.md");
const RESULTS_DIR = path.join(ROOT, "tests", "e2e-results");

const args = parseArgs(process.argv.slice(2));
const baseUrl = String(args["base-url"] ?? "http://127.0.0.1:8964").replace(/\/+$/, "");
const selectedCase = args.case ? String(args.case).padStart(2, "0") : null;
const timeoutMs = Number(args["timeout-ms"] ?? 900_000);
const pollMs = Number(args["poll-ms"] ?? 1_000);
const eventLimit = Number(args["event-limit"] ?? 250);
const reportOnly = Boolean(args["report-only"]);

const startedAt = new Date();
const stamp = timestamp(startedAt);

async function main() {
  await assertGateway();
  const allCases = await loadCases();
  const cases = selectedCase ? allCases.filter((item) => item.id === selectedCase) : allCases;
  if (selectedCase && cases.length === 0) {
    throw new Error(`没有找到 case ${selectedCase}`);
  }

  const results = [];
  for (const testCase of cases) {
    console.log(`运行 case ${testCase.id}: ${testCase.title}`);
    results.push(await runCase(testCase));
  }

  await mkdir(RESULTS_DIR, { recursive: true });
  const payload = {
    started_at: startedAt.toISOString(),
    finished_at: new Date().toISOString(),
    base_url: baseUrl,
    selected_case: selectedCase,
    timeout_ms: timeoutMs,
    poll_ms: pollMs,
    event_limit: eventLimit,
    cases: results,
  };
  const jsonPath = path.join(RESULTS_DIR, `${stamp}.json`);
  const mdPath = path.join(RESULTS_DIR, `${stamp}.md`);
  await writeFile(jsonPath, `${JSON.stringify(payload, null, 2)}\n`);
  await writeFile(mdPath, renderMarkdown(payload));
  console.log(`完成：${path.relative(ROOT, jsonPath)}`);
  console.log(`完成：${path.relative(ROOT, mdPath)}`);
  const failures = collectGateFailures(results);
  if (failures.length > 0) {
    console.error(`E2E gate failed (${failures.length}):`);
    for (const failure of failures) {
      console.error(`- ${failure}`);
    }
    if (!reportOnly) {
      process.exitCode = 1;
    }
  }
}

async function assertGateway() {
  const res = await fetch(`${baseUrl}/api/v1/health`);
  if (!res.ok) {
    throw new Error(`gateway health check failed: HTTP ${res.status}`);
  }
}

async function loadCases() {
  const text = await readFile(CASES_PATH, "utf8");
  const headings = [...text.matchAll(/^##\s+(\d{2})\.\s+(.+)$/gm)];
  return headings.map((match, index) => {
    const [, id, title] = match;
    const bodyStart = match.index + match[0].length;
    const bodyEnd = headings[index + 1]?.index ?? text.length;
    const body = text.slice(bodyStart, bodyEnd);
    const turns = parseTurns(body);
    if (turns.length === 0) {
      throw new Error(`case ${id} 没有解析到 prompt`);
    }
    return { id, title: title.trim(), turns };
  });
}

function parseTurns(body) {
  const turnMatches = [...body.matchAll(/^Turn\s+\d+：\s*\n\n>\s+([\s\S]*?)(?=\n\nTurn\s+\d+：|\n\n主要测试目的：|$)/gm)];
  if (turnMatches.length > 0) {
    return turnMatches.map((match) => normalizeQuote(match[1]));
  }
  const prompt = body.match(/^Prompt：\s*\n\n>\s+([\s\S]*?)(?=\n\n主要测试目的：|$)/m);
  return prompt ? [normalizeQuote(prompt[1])] : [];
}

function normalizeQuote(text) {
  return text
    .split("\n")
    .map((line) => line.replace(/^>\s?/, ""))
    .join("\n")
    .trim();
}

async function runCase(testCase) {
  const sessionId = await createSession(testCase);
  let since = 0;
  const events = [];
  const turns = [];
  const errors = [];

  for (let index = 0; index < testCase.turns.length; index += 1) {
    const turnNumber = index + 1;
    const startedSeq = since;
    const prompt = testCase.turns[index];
    const post = await postTurnWithAutoCompact(sessionId, prompt, events, () =>
      drainEvents(sessionId, since, events)
    );
    since = post.since;

    try {
      const waited = await waitForTurnEnd(sessionId, since, startedSeq);
      since = waited.since;
      events.push(...waited.items);
      turns.push(summarizeTurn(turnNumber, prompt, waited.items));
    } catch (error) {
      const partialItems = error.items ?? [];
      if (partialItems.length > 0) {
        since = error.since ?? since;
        events.push(...partialItems);
      }
      errors.push({ turn: turnNumber, message: error.message });
      await abortSession(sessionId).catch((abortError) => {
        errors.push({ turn: turnNumber, message: `abort failed: ${abortError.message}` });
      });
      turns.push({
        turn: turnNumber,
        prompt,
        terminal_status: "timeout",
        event_count: partialItems.length,
        agent_message_delta_count: countKind(partialItems, "agent_message_delta"),
        agent_message_reset_count: countKind(partialItems, "agent_message_reset"),
        agent_narration_count: countKind(partialItems, "agent_narration"),
        tool_calls: mergeToolCalls(partialItems.filter((item) => item.kind === "tool_call")),
        web_search_count: countKind(partialItems, "web_search_call"),
        errors: [error.message],
        duplicate_same_tool_args: findDuplicateToolCalls(
          mergeToolCalls(partialItems.filter((item) => item.kind === "tool_call"))
        ),
      });
      break;
    }
  }

  const tail = await drainEvents(sessionId, since, events);
  since = tail.since;
  const messages = await listMessages(sessionId);
  const summary = summarizeCase(testCase, sessionId, events, turns, messages, errors);
  return {
    id: testCase.id,
    title: testCase.title,
    session_id: sessionId,
    turns,
    summary,
    messages,
    events,
  };
}

async function createSession(testCase) {
  const id = `eval-${stamp}-${testCase.id}`;
  const res = await fetch(`${baseUrl}/api/v1/sessions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ id, title: `E2E ${testCase.id} ${testCase.title}` }),
  });
  if (!res.ok) {
    throw new Error(`create session ${testCase.id} failed: HTTP ${res.status} ${await res.text()}`);
  }
  const body = await res.json();
  return body.id ?? id;
}

async function postTurnWithAutoCompact(sessionId, prompt, events, drain) {
  let current = await drain();
  for (;;) {
    const res = await fetch(`${baseUrl}/api/v1/sessions/${encodeURIComponent(sessionId)}/messages`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ content: { type: "text", text: prompt } }),
    });
    if (res.status === 201) {
      return { since: current.since };
    }
    if (res.status === 202) {
      current = await waitForKind(sessionId, current.since, "compaction.completed");
      events.push(...current.items);
      continue;
    }
    throw new Error(`post message failed: HTTP ${res.status} ${await res.text()}`);
  }
}

async function waitForTurnEnd(sessionId, since, startedSeq) {
  const deadline = Date.now() + timeoutMs;
  const items = [];
  let cursor = since;
  while (Date.now() < deadline) {
    const drained = await fetchEvents(sessionId, cursor);
    if (drained.items.length > 0) {
      items.push(...drained.items);
      cursor = drained.since;
      const done = items.find(
        (item) => item.seq > startedSeq && item.kind === "agent_message_end"
      );
      if (done) return { since: cursor, items };
    }
    await sleep(pollMs);
  }
  const error = new Error(`等待 agent_message_end 超时：${Math.round(timeoutMs / 1000)}s`);
  error.items = items;
  error.since = cursor;
  throw error;
}

async function waitForKind(sessionId, since, kind) {
  const deadline = Date.now() + timeoutMs;
  let cursor = since;
  const items = [];
  while (Date.now() < deadline) {
    const drained = await fetchEvents(sessionId, cursor);
    if (drained.items.length > 0) {
      items.push(...drained.items);
      cursor = drained.since;
      if (items.some((item) => item.kind === kind)) return { since: cursor, items };
    }
    await sleep(pollMs);
  }
  throw new Error(`等待 ${kind} 超时：${Math.round(timeoutMs / 1000)}s`);
}

async function drainEvents(sessionId, since, sink) {
  let cursor = since;
  for (;;) {
    const drained = await fetchEvents(sessionId, cursor);
    if (drained.items.length === 0) return { since: cursor };
    sink.push(...drained.items);
    cursor = drained.since;
    if (drained.items.length < eventLimit) return { since: cursor };
  }
}

async function fetchEvents(sessionId, since) {
  const url = new URL(`${baseUrl}/api/v1/sessions/${encodeURIComponent(sessionId)}/events`);
  url.searchParams.set("since", String(since));
  url.searchParams.set("limit", String(eventLimit));
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`fetch events failed: HTTP ${res.status} ${await res.text()}`);
  }
  const body = await res.json();
  const items = (body.items ?? []).map(normalizeEvent);
  const last = items.at(-1);
  return { since: last ? last.seq : since, items };
}

async function listMessages(sessionId) {
  const url = new URL(`${baseUrl}/api/v1/sessions/${encodeURIComponent(sessionId)}/messages`);
  url.searchParams.set("limit", "1000");
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`list messages failed: HTTP ${res.status} ${await res.text()}`);
  }
  const body = await res.json();
  return (body.items ?? []).map((row) => ({
    ...row,
    content: parseJson(row.content_json),
  }));
}

async function abortSession(sessionId) {
  const res = await fetch(`${baseUrl}/api/v1/sessions/${encodeURIComponent(sessionId)}/abort`, {
    method: "POST",
  });
  if (res.status !== 202) {
    throw new Error(`HTTP ${res.status} ${await res.text()}`);
  }
}

function normalizeEvent(row) {
  return {
    ...row,
    payload: parseJson(row.payload_json),
  };
}

function summarizeTurn(turn, prompt, events) {
  const end = events.filter((item) => item.kind === "agent_message_end").at(-1);
  const toolCalls = mergeToolCalls(events.filter((item) => item.kind === "tool_call"));
  const duplicateToolCalls = findDuplicateToolCalls(toolCalls);
  return {
    turn,
    prompt,
    terminal_status: end?.payload?.stop_reason ?? null,
    message_seq: end?.payload?.message_seq ?? null,
    event_count: events.length,
    agent_message_delta_count: countKind(events, "agent_message_delta"),
    agent_message_reset_count: countKind(events, "agent_message_reset"),
    agent_narration_count: countKind(events, "agent_narration"),
    web_search_count: countKind(events, "web_search_call"),
    tool_calls: toolCalls,
    duplicate_same_tool_args: duplicateToolCalls,
    errors: collectErrors(events),
  };
}

function summarizeCase(testCase, sessionId, events, turns, messages, errors) {
  const agentMessages = messages.filter((item) => item.role === "agent");
  const finalMessage = agentMessages.at(-1);
  const finalText = String(finalMessage?.content?.text ?? "");
  const toolCalls = mergeToolCalls(events.filter((item) => item.kind === "tool_call"));
  return {
    session_id: sessionId,
    turn_count: testCase.turns.length,
    completed_turns: turns.filter((turn) => turn.terminal_status && turn.terminal_status !== "timeout").length,
    terminal_statuses: turns.map((turn) => turn.terminal_status),
    message_count: messages.length,
    event_count: events.length,
    final_message_seq: finalMessage?.seq ?? null,
    final_message_text: finalText,
    final_message_preview: preview(finalText, 800),
    tool_call_count: toolCalls.length,
    tool_calls: toolCalls,
    duplicate_same_tool_args: findDuplicateToolCalls(toolCalls),
    web_search_count: countKind(events, "web_search_call"),
    agent_message_delta_count: countKind(events, "agent_message_delta"),
    agent_message_reset_count: countKind(events, "agent_message_reset"),
    agent_narration_count: countKind(events, "agent_narration"),
    errors: [...errors, ...collectErrors(events)],
  };
}

function mergeToolCalls(events) {
  const byId = new Map();
  for (const event of events) {
    const payload = event.payload ?? {};
    const callId = String(payload.call_id ?? "");
    const key = callId || `${event.seq}:${payload.name ?? ""}`;
    const previous = byId.get(key) ?? {};
    byId.set(key, {
      call_id: callId,
      name: payload.name ?? previous.name ?? "",
      arguments: payload.arguments ?? previous.arguments ?? null,
      status: payload.status ?? previous.status ?? null,
      output_preview: payload.output_preview ?? previous.output_preview ?? null,
      output_bytes: payload.output_bytes ?? previous.output_bytes ?? null,
      duration_ms: payload.duration_ms ?? previous.duration_ms ?? null,
      cached: payload.cached ?? previous.cached ?? null,
      cached_from: payload.cached_from ?? previous.cached_from ?? null,
      error_kind: payload.error_kind ?? previous.error_kind ?? null,
      retryable: payload.retryable ?? previous.retryable ?? null,
      first_seq: previous.first_seq ?? event.seq,
      last_seq: event.seq,
    });
  }
  return [...byId.values()];
}

function findDuplicateToolCalls(toolCalls) {
  const groups = new Map();
  for (const call of toolCalls) {
    const key = `${call.name}\n${normalizeArgs(call.arguments)}`;
    const group = groups.get(key) ?? [];
    group.push(call.call_id || `seq:${call.first_seq}`);
    groups.set(key, group);
  }
  return [...groups.entries()]
    .filter(([, ids]) => ids.length > 1)
    .map(([key, call_ids]) => {
      const [name, args] = key.split("\n");
      return { name, arguments: args, count: call_ids.length, call_ids };
    });
}

function normalizeArgs(value) {
  if (value === null || value === undefined) return "";
  if (typeof value !== "string") return JSON.stringify(value);
  const parsed = parseJson(value);
  if (!parsed || typeof parsed !== "object") return value.trim();
  return JSON.stringify(sortJson(parsed));
}

function sortJson(value) {
  if (Array.isArray(value)) return value.map(sortJson);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sortJson(value[key])]));
}

function collectErrors(events) {
  const out = [];
  for (const event of events) {
    const payload = event.payload ?? {};
    if (
      event.kind === "provider_error" ||
      event.kind === "provider_retry" ||
      event.kind === "agent_message_failed" ||
      event.kind === "error"
    ) {
      out.push({ seq: event.seq, kind: event.kind, message: payload.error ?? payload.message ?? "" });
    }
    if (event.kind === "tool_call" && payload.status === "error") {
      out.push({
        seq: event.seq,
        kind: "tool_call",
        name: payload.name ?? "",
        error_kind: payload.error_kind ?? null,
        retryable: payload.retryable ?? null,
        message: payload.output_preview ?? "",
      });
    }
  }
  return out;
}

function collectGateFailures(results) {
  const failures = [];
  for (const item of results) {
    const s = item.summary;
    if (s.completed_turns !== s.turn_count) {
      failures.push(`${item.id}: completed ${s.completed_turns}/${s.turn_count} turns`);
    }
    if ((s.errors ?? []).length > 0) {
      failures.push(`${item.id}: ${s.errors.length} error events`);
    }
    if ((s.duplicate_same_tool_args ?? []).length > 0) {
      failures.push(`${item.id}: ${s.duplicate_same_tool_args.length} duplicate same tool+args groups`);
    }
    for (const status of s.terminal_statuses ?? []) {
      if (!isSuccessfulTerminal(status)) {
        failures.push(`${item.id}: terminal status ${status ?? "missing"}`);
      }
    }
  }
  return failures;
}

function isSuccessfulTerminal(status) {
  return status === "end_turn" ||
    status === "endturn" ||
    status === "awaiting_user" ||
    status === "max_tool_turns_finalized";
}

function renderMarkdown(payload) {
  const lines = [];
  lines.push("# A 股 API E2E Eval Summary");
  lines.push("");
  lines.push(`- 开始时间：${payload.started_at}`);
  lines.push(`- 结束时间：${payload.finished_at}`);
  lines.push(`- Gateway：${payload.base_url}`);
  lines.push(`- Case：${payload.selected_case ?? "全部"}`);
  lines.push("");
  lines.push("| Case | Session | Turns | Terminal | Tools | Web Search | Delta | Reset | Narration | Errors | Duplicate Tool Args |");
  lines.push("| --- | --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
  for (const item of payload.cases) {
    const s = item.summary;
    lines.push(
      `| ${item.id} | \`${item.session_id}\` | ${s.completed_turns}/${s.turn_count} | ${s.terminal_statuses.join(", ")} | ${s.tool_call_count} | ${s.web_search_count} | ${s.agent_message_delta_count} | ${s.agent_message_reset_count} | ${s.agent_narration_count} | ${s.errors.length} | ${s.duplicate_same_tool_args.length} |`
    );
  }
  lines.push("");
  for (const item of payload.cases) {
    lines.push(`## ${item.id}. ${item.title}`);
    lines.push("");
    lines.push(`Session: \`${item.session_id}\``);
    lines.push("");
    for (const turn of item.turns) {
      lines.push(`### Turn ${turn.turn}`);
      lines.push("");
      lines.push(`- Terminal：${turn.terminal_status ?? "无"}`);
      lines.push(`- Events：${turn.event_count}`);
      lines.push(`- Tool calls：${turn.tool_calls.length}`);
      lines.push(`- Web search events：${turn.web_search_count}`);
      lines.push(`- Delta / Reset / Narration：${turn.agent_message_delta_count} / ${turn.agent_message_reset_count} / ${turn.agent_narration_count}`);
      const turnToolCalls = turn.tool_calls ?? [];
      const turnErrors = turn.errors ?? [];
      const turnDuplicates = turn.duplicate_same_tool_args ?? [];
      if (turnToolCalls.length > 0) {
        lines.push("");
        lines.push("| Tool | Status | Args | Output bytes | Cached |");
        lines.push("| --- | --- | --- | ---: | --- |");
        for (const call of turnToolCalls) {
          lines.push(`| ${call.name} | ${call.status ?? ""} | \`${escapeCell(preview(normalizeArgs(call.arguments), 160))}\` | ${call.output_bytes ?? ""} | ${call.cached ?? ""} |`);
        }
      }
      if (turnErrors.length > 0) {
        lines.push("");
        lines.push("错误：");
        for (const error of turnErrors) {
          lines.push(`- ${error.kind ?? "runner"} ${error.name ?? ""} ${preview(error.message ?? "", 240)}`);
        }
      }
      if (turnDuplicates.length > 0) {
        lines.push("");
        lines.push("重复 same tool+args：");
        for (const dup of turnDuplicates) {
          lines.push(`- ${dup.name} × ${dup.count} \`${escapeCell(preview(dup.arguments, 180))}\``);
        }
      }
      lines.push("");
    }
    lines.push("最终消息预览：");
    lines.push("");
    lines.push("```text");
    lines.push(item.summary.final_message_preview || "");
    lines.push("```");
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const item = argv[i];
    if (!item.startsWith("--")) continue;
    const key = item.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      out[key] = true;
    } else {
      out[key] = next;
      i += 1;
    }
  }
  return out;
}

function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function countKind(events, kind) {
  return events.filter((item) => item.kind === kind).length;
}

function preview(text, max) {
  const value = String(text ?? "").replace(/\s+/g, " ").trim();
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function escapeCell(text) {
  return String(text ?? "").replaceAll("|", "\\|").replaceAll("\n", " ");
}

function timestamp(date) {
  return date.toISOString().replace(/[:.]/g, "-").replace("T", "_").replace("Z", "Z");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}
