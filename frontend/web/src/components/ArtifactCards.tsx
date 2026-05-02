// Tool-specific canvas cards. Each tool kind gets its own renderer that
// presents the output as the user wants to *see* it (a wiki abstract, a
// quote chip, a filings list) rather than raw JSON. When we can't parse
// the truncated output (or the tool isn't specifically handled yet), a
// generic fallback card shows the bare facts.

import { For, Show } from "solid-js";
import { SafeMarkdown } from "./SafeMarkdown";

export interface ToolCallView {
  call_id: string;
  status: "in_progress" | "completed" | "error" | string;
  name: string;
  arguments?: string;
  output_preview?: string;
  output_bytes?: number;
}

export interface SearchCallView {
  status: string;
  action: string;
  detail: string;
}

export interface NarrationStep {
  turn: number;
  text: string;
}

interface ArtifactCallbacks {
  onOpenDoc?: (id: string, title: string) => void;
}

export function ArtifactPanel(props: {
  searches: SearchCallView[];
  tools: ToolCallView[];
  narrations?: NarrationStep[];
  callbacks?: ArtifactCallbacks;
}) {
  return (
    <div style={{
      position: "absolute",
      top: "70px",
      left: "36px",
      // Match the prototype's "stacked panel column" feel — a fixed-width
      // ribbon down the left of the canvas, brain stays on the right.
      width: "min(520px, 56%)",
      bottom: "20px",
      "padding-right": "8px",
      overflow: "auto",
      display: "flex",
      "flex-direction": "column",
      gap: "10px",
    }}>
      <For each={props.narrations ?? []}>{(n) => <NarrationCard step={n} />}</For>
      <For each={props.searches}>{(s) => <SearchArtifact search={s} />}</For>
      <For each={props.tools}>{(t) => (
        <ToolArtifact tool={t} callbacks={props.callbacks} />
      )}</For>
    </div>
  );
}

function NarrationCard(props: { step: NarrationStep }) {
  return (
    <div style={{
      "border-left": "3px solid var(--clay)",
      padding: "8px 14px",
      background: "rgba(217, 119, 87, 0.04)",
      "border-radius": "0 6px 6px 0",
      "font-size": "12.5px",
      "line-height": 1.55,
      color: "var(--ink-1)",
    }}>
      <div style={{
        "font-family": "var(--font-mono)",
        "font-size": "10px",
        "text-transform": "uppercase",
        "letter-spacing": "0.06em",
        color: "var(--clay-soft)",
        "margin-bottom": "4px",
      }}>
        thinking · turn {props.step.turn + 1}
      </div>
      <SafeMarkdown source={props.step.text} />
    </div>
  );
}

function StatusDot(props: { status: string }) {
  const color = () =>
    props.status === "completed" ? "#9eb78f" :
    props.status === "error" ? "#d97070" : "#d9a76c";
  return (
    <span style={{
      width: "6px",
      height: "6px",
      "border-radius": "50%",
      background: color(),
      display: "inline-block",
      "margin-right": "8px",
      "flex-shrink": 0,
    }} />
  );
}

function CardShell(props: {
  status: string;
  badge: string;
  detail?: string;
  children: any;
  onClick?: () => void;
}) {
  return (
    <div
      onClick={props.onClick}
      style={{
        background: "var(--bg-1)",
        border: "1px solid var(--bg-2)",
        "border-radius": "8px",
        padding: "12px 14px",
        cursor: props.onClick ? "pointer" : "default",
        transition: "border-color 120ms",
      }}
    >
      <div style={{
        display: "flex",
        "align-items": "center",
        gap: "0",
        "margin-bottom": "8px",
        "font-family": "var(--font-mono)",
        "font-size": "10.5px",
        color: "var(--ink-3)",
        "text-transform": "uppercase",
        "letter-spacing": "0.04em",
      }}>
        <StatusDot status={props.status} />
        <span style={{ color: "var(--ink-2)", "font-weight": 600 }}>{props.badge}</span>
        <Show when={props.detail}>
          <span style={{ "margin-left": "10px", "text-transform": "none", "letter-spacing": "0" }}>
            {props.detail}
          </span>
        </Show>
      </div>
      {props.children}
    </div>
  );
}

function SearchArtifact(props: { search: SearchCallView }) {
  const verb = () => props.search.status === "completed"
    ? props.search.action === "open_page" ? "Opened page"
    : props.search.action === "find_in_page" ? "Searched in page"
    : "Searched"
    : "Searching";
  let host = props.search.detail;
  try { host = new URL(props.search.detail).hostname; } catch {/* leave as is */}
  return (
    <CardShell status={props.search.status} badge="web · search" detail={`${verb()}: ${host || "…"}`}>
      <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
        Server-side discovery via codex `web_search`.
      </div>
    </CardShell>
  );
}

function ToolArtifact(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  switch (props.tool.name) {
    case "corpus_search": return <CorpusSearchCard {...props} />;
    case "web_fetch":     return <WebFetchCard {...props} />;
    case "tradingview_quote": return <TradingViewCard {...props} />;
    case "tushare_quote": return <TushareCard {...props} />;
    case "sec_filing_fetch": return <SecFilingCard {...props} />;
    default:              return <GenericToolCard {...props} />;
  }
}

function safeParseJson(s: string | undefined): unknown {
  if (!s) return null;
  try { return JSON.parse(s); }
  catch {
    // Truncated JSON — try shaving the tail until we get a parseable prefix.
    for (let i = s.length - 1; i > 32; i--) {
      const tail = s.slice(0, i);
      // Find a likely safe closing point: balance braces by trimming after
      // the last complete `}]}` or `}}` sequence we can identify.
      try { return JSON.parse(tail); } catch {/* keep shaving */}
    }
    return null;
  }
}

function parseArgs(t: ToolCallView): Record<string, unknown> {
  if (!t.arguments) return {};
  try { return JSON.parse(t.arguments); }
  catch { return {}; }
}

/* ---------- corpus_search ---------- */

interface CorpusHit {
  id: string;
  title: string;
  tier: string;
  layer: string;
  tags?: string[];
  score?: number;
  snippet?: string;
}

function CorpusSearchCard(props: { tool: ToolCallView; callbacks?: ArtifactCallbacks }) {
  const args = parseArgs(props.tool);
  const query = String(args.query ?? "");
  const parsed = safeParseJson(props.tool.output_preview ?? "") as
    | { hits?: CorpusHit[]; total_corpus_docs?: number } | null;
  const hits = parsed?.hits ?? [];
  return (
    <CardShell
      status={props.tool.status}
      badge="corpus"
      detail={query ? `"${query}"` : undefined}
    >
      <Show
        when={hits.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No hits.">
              Searching the corpus…
            </Show>
          </div>
        }
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "10px" }}>
          <For each={hits.slice(0, 4)}>{(h) => (
            <CorpusHitTile hit={h} onOpen={() =>
              props.callbacks?.onOpenDoc?.(h.id, h.title || h.id)
            } />
          )}</For>
        </div>
      </Show>
    </CardShell>
  );
}

function CorpusHitTile(props: { hit: CorpusHit; onOpen: () => void }) {
  const tierLabel = () => `${props.hit.tier}·${props.hit.layer}`;
  return (
    <div
      onClick={props.onOpen}
      title="Click to open the full wiki page"
      style={{
        cursor: "pointer",
        padding: "6px 10px",
        background: "rgba(255, 255, 255, 0.025)",
        border: "1px solid var(--line-1)",
        "border-radius": "5px",
        display: "flex",
        "align-items": "center",
        gap: "8px",
        "min-height": "28px",
      }}
    >
      <span style={{
        "font-size": "12.5px",
        color: "var(--ink-0)",
        flex: 1,
        overflow: "hidden",
        "text-overflow": "ellipsis",
        "white-space": "nowrap",
      }}>{props.hit.title}</span>
      <span style={{
        "font-family": "var(--font-mono)",
        "font-size": "9.5px",
        color: "var(--ink-3)",
        "flex-shrink": 0,
      }}>{tierLabel()}</span>
    </div>
  );
}

/* ---------- web_fetch ---------- */

function WebFetchCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const url = String(args.url ?? "");
  let host = url;
  try { host = new URL(url).hostname.replace(/^www\./, ""); } catch {/* leave */}
  const preview = props.tool.output_preview ?? "";
  // The fetched markdown is preceded by "# Source: <url>\n_(extraction: ...)_\n\n"
  // header — strip it for display so the user sees the page content first.
  const body = preview
    .replace(/^# Source:[^\n]*\n/, "")
    .replace(/^_\(extraction:[^\n]*\)_\n\n/m, "");
  return (
    <CardShell
      status={props.tool.status}
      badge="web · fetch"
      detail={host}
      onClick={url ? () => window.open(url, "_blank", "noopener,noreferrer") : undefined}
    >
      <Show
        when={body.trim()}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No content.">
              Fetching {host}…
            </Show>
          </div>
        }
      >
        <div style={{
          "max-height": "180px",
          overflow: "hidden",
          position: "relative",
          "font-size": "12px",
          "line-height": 1.55,
        }}>
          <SafeMarkdown source={body.slice(0, 1200)} />
          <div style={{
            position: "absolute",
            inset: "auto 0 0 0",
            height: "60px",
            background: "linear-gradient(to bottom, transparent, var(--bg-1) 96%)",
            "pointer-events": "none",
          }} />
        </div>
      </Show>
    </CardShell>
  );
}

/* ---------- tradingview_quote ---------- */

function TradingViewCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tickers = Array.isArray(args.tickers) ? (args.tickers as string[]) : [];
  return (
    <CardShell
      status={props.tool.status}
      badge="market · tradingview"
      detail={tickers.join(", ")}
    >
      <Show
        when={props.tool.output_preview}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            Fetching quotes…
          </div>
        }
      >
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "11.5px",
          "white-space": "pre-wrap",
          color: "var(--ink-2)",
          "max-height": "200px",
          overflow: "hidden",
          position: "relative",
        }}>
          <SafeMarkdown source={props.tool.output_preview ?? ""} />
          <div style={{
            position: "absolute",
            inset: "auto 0 0 0",
            height: "40px",
            background: "linear-gradient(to bottom, transparent, var(--bg-1) 96%)",
            "pointer-events": "none",
          }} />
        </div>
      </Show>
    </CardShell>
  );
}

/* ---------- tushare_quote ---------- */

function TushareCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const tsCode = String(args.ts_code ?? "");
  return (
    <CardShell
      status={props.tool.status}
      badge="market · A 股"
      detail={tsCode}
    >
      <Show
        when={props.tool.output_preview}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            Fetching A-share OHLCV…
          </div>
        }
      >
        <div style={{
          "max-height": "200px",
          overflow: "hidden",
          position: "relative",
          "font-size": "11.5px",
        }}>
          <SafeMarkdown source={props.tool.output_preview ?? ""} />
          <div style={{
            position: "absolute",
            inset: "auto 0 0 0",
            height: "40px",
            background: "linear-gradient(to bottom, transparent, var(--bg-1) 96%)",
            "pointer-events": "none",
          }} />
        </div>
      </Show>
    </CardShell>
  );
}

/* ---------- sec_filing_fetch ---------- */

interface SecFiling {
  form: string;
  filed: string;
  primary_doc_url: string;
  description?: string;
}

function SecFilingCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const ticker = String(args.ticker ?? "").toUpperCase();
  const parsed = safeParseJson(props.tool.output_preview ?? "") as
    | { filings?: SecFiling[]; company_name?: string } | null;
  const filings = parsed?.filings ?? [];
  return (
    <CardShell
      status={props.tool.status}
      badge="sec · edgar"
      detail={parsed?.company_name ? `${ticker} · ${parsed.company_name}` : ticker}
    >
      <Show
        when={filings.length > 0}
        fallback={
          <div style={{ color: "var(--ink-3)", "font-size": "11.5px" }}>
            <Show when={props.tool.status === "in_progress"} fallback="No filings.">
              Fetching filings…
            </Show>
          </div>
        }
      >
        <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
          <For each={filings.slice(0, 5)}>{(f) => (
            <a
              href={f.primary_doc_url}
              target="_blank"
              rel="noopener noreferrer"
              style={{
                display: "flex",
                gap: "10px",
                padding: "6px 10px",
                background: "rgba(255, 255, 255, 0.025)",
                border: "1px solid var(--line-1)",
                "border-radius": "6px",
                "font-family": "var(--font-mono)",
                "font-size": "11px",
                color: "var(--ink-1)",
                "text-decoration": "none",
              }}
            >
              <span style={{ "min-width": "50px", color: "var(--clay-soft)" }}>{f.form}</span>
              <span style={{ "min-width": "100px", color: "var(--ink-3)" }}>{f.filed}</span>
              <span style={{ flex: 1, "white-space": "nowrap", overflow: "hidden", "text-overflow": "ellipsis" }}>
                {f.description || f.primary_doc_url.split("/").slice(-1)[0]}
              </span>
            </a>
          )}</For>
        </div>
      </Show>
    </CardShell>
  );
}

/* ---------- generic fallback ---------- */

function GenericToolCard(props: { tool: ToolCallView }) {
  const args = parseArgs(props.tool);
  const summary = (() => {
    if (typeof args.url === "string") return String(args.url);
    if (typeof args.query === "string") return String(args.query);
    if (typeof args.ts_code === "string") return String(args.ts_code);
    if (Array.isArray(args.tickers)) return (args.tickers as string[]).join(", ");
    return JSON.stringify(args).slice(0, 60);
  })();
  return (
    <CardShell status={props.tool.status} badge={props.tool.name} detail={summary}>
      <Show when={props.tool.output_preview}>
        <div style={{
          "font-family": "var(--font-mono)",
          "font-size": "11px",
          color: "var(--ink-2)",
          "white-space": "pre-wrap",
          "max-height": "120px",
          overflow: "hidden",
        }}>{props.tool.output_preview}</div>
      </Show>
    </CardShell>
  );
}
