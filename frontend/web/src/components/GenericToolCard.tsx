// GenericToolCard — Phase 1 placeholder for every tool card.
//
// Until Phase 2 lands per-tool renderers (stock_overview tabs / market_pulse
// quote rows / chart_data k-line), every tool / search / note / subagent
// artifact draws through this single component. It does three things:
//   1. picks size + icon from artifact kind / cardKind
//   2. renders the distilled markdown the gateway packed into
//      `display_payload.markdown` / `display_payload.text` (the M1.9 contract
//      already includes a "good enough for chat" rendering target)
//   3. lets the user toggle a raw-JSON view so even early kinds we don't
//      special-case stay debuggable
//
// State coverage:
//   - phase = start  → "运行中…" line + spinning dot (loading)
//   - phase = error  → red-tinted error message (error)
//   - phase = completion + content → markdown (populated)
//   - phase = completion + no content → "无可显示内容" (empty)
//   - raw toggle    → covers debug / edge case

import { createSignal, For, Match, Show, Switch } from "solid-js";

import { SafeMarkdown } from "./SafeMarkdown";
import { CardShell, type CardSize, type CardStatus } from "./CardShell";
import type { IconName } from "./Icon";
import type { Artifact } from "../types";

type Props = {
  artifact: Artifact;
  /** True when the chat summary just clicked into this card. */
  highlighted?: boolean;
};

/** Default size class per artifact kind. Phase 2's per-tool renderers
 *  override these with their own knowledge — e.g. chart_data goes "full",
 *  market_pulse goes "sm". Subagent cards default to "lg" because they
 *  carry nested artifacts. */
function defaultSize(a: Artifact): CardSize {
  if (a.kind === "note") return "md";
  if (a.kind === "search") return "lg";
  if (a.kind === "subagent") return "lg";
  if (a.kind === "codex_duplicate_warning") return "md";
  const ck = a.cardKind ?? "";
  if (ck === "chart_data") return "full";
  if (ck === "market_pulse" || ck === "quote") return "sm";
  if (ck === "web_preview") return "lg";
  return "md";
}

/** Pick a header icon from the artifact + cardKind. Keeps the mapping
 *  short — Phase 2 widens it as tool cards specialise. */
function iconFor(a: Artifact): IconName {
  if (a.kind === "note") return "spark";
  if (a.kind === "search") return "search";
  if (a.kind === "subagent") return "branch";
  if (a.kind === "codex_duplicate_warning") return "eye";
  return "dot";
}

/** Map artifact phase + kind into a CardStatus the shell can render. */
function statusFor(a: Artifact): CardStatus {
  if (a.phase === "start") return "running";
  if (a.phase === "error") return "danger";
  if (a.kind === "codex_duplicate_warning") {
    return a.warningAborted ? "danger" : "warn";
  }
  return "ok";
}

/** Build the card title from the artifact's natural identifier. */
function titleFor(a: Artifact): string {
  if (a.kind === "note") return "Note Trace";
  if (a.kind === "search") {
    const action = a.actionType ?? "search";
    if (action === "search") return "网页搜索";
    if (action === "open_page") return "打开网页";
    if (action === "find_in_page") return "页面内查找";
    return `网页活动 · ${action}`;
  }
  if (a.kind === "subagent") {
    return `委派给 ${a.agentName ?? "subagent"}`;
  }
  if (a.kind === "codex_duplicate_warning") {
    return "codex 重复 URL 警告";
  }
  return a.displayName ?? a.tool ?? "工具";
}

/** Card subtitle — small mono-tag for the technical identity. */
function subtitleFor(a: Artifact): string | undefined {
  if (a.kind === "tool") return a.cardKind ?? a.tool;
  if (a.kind === "search") return a.actionType;
  if (a.kind === "subagent" && a.depth != null) return `depth ${a.depth}`;
  return undefined;
}

/** Extract a "good enough for chat" markdown summary from the
 *  display_payload. Each tool packs its own; we pick the first available
 *  field in a hierarchy. */
function distilledMarkdown(a: Artifact): string {
  if (a.kind === "note") return a.text ?? "";
  if (a.kind === "search") {
    if (a.actionType === "open_page" && a.pageSnippet) return a.pageSnippet;
    if (a.summary) return a.summary;
    if (a.query) return `**查询**: ${a.query}`;
    return "";
  }
  if (a.kind === "subagent") {
    if (a.resultPreview) return a.resultPreview;
    if (a.inputPreview) return `**输入**: ${a.inputPreview}`;
    return "";
  }
  if (a.kind === "codex_duplicate_warning") {
    const url = a.warningUrl ?? "";
    const action = a.warningActionType ?? "?";
    const count = a.warningCount ?? 0;
    const aborted = a.warningAborted ? "已强制中止当前 iter" : "已提示模型停止重复 open";
    return `\`${action}\` 第 ${count} 次访问 \`${url}\`\n\n${aborted}`;
  }
  // Tool — preferred fields in order. Phase 2 per-tool renderers know
  // each tool's actual payload shape; we lean on the gateway's
  // "summary"/markdown packs here as a graceful fallback.
  const dp = (a.displayPayload ?? {}) as Record<string, unknown>;
  if (typeof dp.markdown === "string") return dp.markdown;
  if (typeof dp.text === "string") return dp.text;
  if (typeof a.summary === "string" && a.summary.length > 0) return a.summary;
  return "";
}

function formatJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}

export function GenericToolCard(props: Props) {
  const [raw, setRaw] = createSignal(false);

  const size = () => defaultSize(props.artifact);
  const icon = () => iconFor(props.artifact);
  const status = () => statusFor(props.artifact);
  const title = () => titleFor(props.artifact);
  const sub = () => subtitleFor(props.artifact);
  const body = () => distilledMarkdown(props.artifact);
  const isStart = () => props.artifact.phase === "start";
  const isError = () => props.artifact.phase === "error";

  const errorText = () => {
    const dp = (props.artifact.displayPayload ?? {}) as Record<string, unknown>;
    if (typeof dp.error === "string") return dp.error;
    if (props.artifact.errorMessage) return props.artifact.errorMessage;
    return "工具调用失败";
  };

  return (
    <CardShell
      id={props.artifact.artifactId}
      size={size()}
      icon={icon()}
      status={status()}
      title={title()}
      subtitle={sub()}
      highlighted={props.highlighted}
      actions={[
        {
          id: "raw",
          icon: raw() ? "chevronD" : "chevronR",
          label: raw() ? "隐藏 raw JSON" : "显示 raw JSON",
          onClick: () => setRaw((v) => !v),
        },
      ]}
    >
      <Switch>
        <Match when={isStart()}>
          <p class="lk-card-loading">
            <span class="lk-pulse lk-card-loading-dot" aria-hidden="true" />
            {props.artifact.kind === "subagent" ? "subagent 运行中…" : "运行中…"}
          </p>
        </Match>
        <Match when={isError()}>
          <p class="lk-card-error">
            <span aria-hidden="true">✗ </span>
            {errorText()}
          </p>
        </Match>
        <Match when={body().length > 0}>
          <SafeMarkdown text={body()} class="lk-card-md" />
        </Match>
        <Match when={true}>
          <p class="lk-card-empty">无可显示内容</p>
        </Match>
      </Switch>
      <Show when={raw()}>
        <pre class="lk-card-raw lk-mono">{formatJson({
          kind: props.artifact.kind,
          phase: props.artifact.phase,
          iteration: props.artifact.iteration,
          tool: props.artifact.tool,
          cardKind: props.artifact.cardKind,
          summary: props.artifact.summary,
          displayPayload: props.artifact.displayPayload,
        })}</pre>
      </Show>
      {/* Phase 2 will replace subagent recursion with a proper renderer;
          for Phase 1 we list inner artifacts as a flat ordered list so the
          structure is visible. */}
      <Show
        when={
          props.artifact.kind === "subagent" &&
          props.artifact.innerArtifacts &&
          props.artifact.innerArtifacts.length > 0
        }
      >
        <details class="lk-card-subagent-inner">
          <summary>
            内部步骤 ({props.artifact.innerArtifacts!.length})
          </summary>
          <ol class="lk-subagent-inner-list">
            <For each={props.artifact.innerArtifacts}>
              {(inner) => (
                <li>
                  <GenericToolCard artifact={inner} />
                </li>
              )}
            </For>
          </ol>
        </details>
      </Show>
    </CardShell>
  );
}
