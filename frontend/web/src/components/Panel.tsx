// Composable panel system. A panel = chrome-wrapped stack of typed modules.
// Ported from prototype/leek-panels.jsx → SolidJS.

import { For, Show, createMemo, type JSX } from "solid-js";
import { Icon } from "./Icon";

/* ============= types ============= */

export type PanelKind =
  | "quote-card" | "fundamentals" | "chart" | "evidence" | "corpus"
  | "compare" | "news" | "subagent" | "valuation" | "decision" | "watch";

export type Module =
  | { kind: "quote"; data: QuoteData }
  | { kind: "kv"; title?: string; rows: Array<[string, string, string?]> }
  | { kind: "candles"; w: number; h: number; seed?: number; base?: number; sym?: string; price?: number; range?: string }
  | { kind: "pdf"; doc: PdfDoc; snippets: PdfSnippet[] }
  | { kind: "cites"; items: CiteItem[] }
  | { kind: "news"; items: NewsItem[] }
  | { kind: "rows"; rows: WatchRow[] }
  | { kind: "cmp"; headers: string[]; rows: CmpRow[] }
  | { kind: "sub"; data: SubAgentData }
  | { kind: "valuation"; steps: ValuationStep[]; target?: number; current?: number }
  | { kind: "decision"; data: DecisionData };

export interface QuoteData { sym: string; price: number; chg: number; chgPct: number; ts: string; venue: string; }
export interface PdfDoc { title: string; intro: string; body1: string; highlight?: string; body2: string; body3: string; lbl: string; }
export interface PdfSnippet { before: string; mark: string; after: string; cite: string; }
export interface CiteItem { tier: string; path: string; title: string; quote?: string; }
export interface NewsItem { ts: string; src: string; head: string; imp: string; }
export interface WatchRow { sym: string; name: string; px: number; ch: number; }
export interface CmpCell { v: string; cls?: string; }
export interface CmpRow { hl?: boolean; cells: CmpCell[]; }
export interface SubAgentData { role: string; task: string; progress: number; step: number; total: number; tools: number; elapsed: string; }
export interface ValuationStep { k: string; v: string; tot?: boolean; }
export interface DecisionData { verdict: string; sym: string; confidence: number; gist: string; params: Array<{ k: string; v: string }>; }

/* ============= module renderers ============= */

function ModQuote(props: { data: QuoteData }) {
  const ch = props.data.chgPct;
  return (
    <div class="lk-mod-quote">
      <span class="sym">{props.data.sym}</span>
      <span class="px">{props.data.price.toFixed(2)}</span>
      <span class={"ch " + (ch >= 0 ? "up" : "dn")}>
        {ch >= 0 ? "+" : ""}{props.data.chg.toFixed(2)} ({ch >= 0 ? "+" : ""}{ch.toFixed(2)}%)
      </span>
      <span class="meta">{props.data.venue} · {props.data.ts}</span>
    </div>
  );
}

function ModKV(props: { title?: string; rows: Array<[string, string, string?]> }) {
  return (
    <div class="lk-mod lk-mod-kv">
      <Show when={props.title}><div class="lk-mod-head">{props.title}</div></Show>
      <table>
        <tbody>
          <For each={props.rows}>{(r) => (
            <tr><td>{r[0]}</td><td class={r[2] ?? ""}>{r[1]}</td></tr>
          )}</For>
        </tbody>
      </table>
    </div>
  );
}

/* deterministic candle generator — same shape regardless of caller */
function genCandles(seed: number, n: number, base: number) {
  let s = seed;
  const rand = () => { s = (s * 9301 + 49297) % 233280; return s / 233280; };
  const out: Array<{ open: number; close: number; hi: number; lo: number; vol: number }> = [];
  let close = base;
  for (let i = 0; i < n; i++) {
    const drift = (rand() - 0.40) * 11;
    const open = close;
    close = Math.max(20, open + drift);
    const hi = Math.max(open, close) + rand() * 5.2;
    const lo = Math.min(open, close) - rand() * 5.2;
    const vol = 0.35 + rand() * 0.6;
    out.push({ open, close, hi, lo, vol });
  }
  return out;
}

function ModCandles(props: { w: number; h: number; seed?: number; base?: number; sym?: string; price?: number }) {
  const seed = props.seed ?? 7;
  const base = props.base ?? 880;
  const candles = createMemo(() => genCandles(seed, 64, base));
  const W = props.w, H = props.h;
  const padL = 10, padR = 48, padT = 8, padB = 26;
  const innerW = W - padL - padR;
  const innerH = H - padT - padB;
  const volH = 22;
  const priceH = innerH - volH - 4;

  const layout = createMemo(() => {
    const cs = candles();
    let lo = Infinity, hi = -Infinity;
    for (const c of cs) { lo = Math.min(lo, c.lo); hi = Math.max(hi, c.hi); }
    const pad = (hi - lo) * 0.10; lo -= pad; hi += pad;
    const xStep = innerW / cs.length;
    const yScale = (p: number) => padT + (1 - (p - lo) / (hi - lo)) * priceH;
    const bodyW = Math.max(1.6, xStep * 0.60);
    const ticks = 4;
    const tickVals = Array.from({ length: ticks + 1 }, (_, i) => lo + (hi - lo) * (i / ticks));
    const last = cs[cs.length - 1].close;
    const px = props.price ?? last;
    return { cs, lo, hi, xStep, yScale, bodyW, tickVals, px };
  });

  const xLabels = ["MAR", "APR", "MAY", "JUN", "JUL"];

  return (
    <div class="lk-mod lk-mod-chart">
      <svg width={W} height={H} style={{ display: "block" }}>
        <defs>
          <linearGradient id={`cup-${seed}`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stop-color="#6fb98a" stop-opacity="1" />
            <stop offset="100%" stop-color="#6fb98a" stop-opacity="0.85" />
          </linearGradient>
          <linearGradient id={`cdn-${seed}`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stop-color="#d97070" stop-opacity="1" />
            <stop offset="100%" stop-color="#d97070" stop-opacity="0.85" />
          </linearGradient>
        </defs>

        <For each={layout().tickVals}>{(v) => (
          <g>
            <line x1={padL} x2={W - padR} y1={layout().yScale(v)} y2={layout().yScale(v)} stroke="rgba(255,240,220,0.05)" />
            <text x={W - padR + 5} y={layout().yScale(v) + 3}
              fill="rgba(168,158,140,0.65)" font-size="9" font-family="JetBrains Mono, monospace">
              {v.toFixed(0)}
            </text>
          </g>
        )}</For>

        <For each={layout().cs}>{(c, i) => {
          const cx = padL + layout().xStep * (i() + 0.5);
          const up = c.close >= c.open;
          const yO = layout().yScale(c.open);
          const yC = layout().yScale(c.close);
          const yH = layout().yScale(c.hi);
          const yL = layout().yScale(c.lo);
          const fill = up ? `url(#cup-${seed})` : `url(#cdn-${seed})`;
          const stroke = up ? "#6fb98a" : "#d97070";
          return (
            <g>
              <line x1={cx} x2={cx} y1={yH} y2={yL} stroke={stroke} stroke-width="0.7" />
              <rect
                x={cx - layout().bodyW / 2}
                y={Math.min(yO, yC)}
                width={layout().bodyW}
                height={Math.max(1, Math.abs(yC - yO))}
                fill={fill}
                stroke={stroke}
                stroke-width="0.4"
              />
            </g>
          );
        }}</For>

        <path
          d={layout().cs.map((c, i) => `${i === 0 ? "M" : "L"} ${padL + layout().xStep * (i + 0.5)} ${layout().yScale(c.close)}`).join(" ")}
          fill="none" stroke="rgba(217,119,87,0.6)" stroke-width="0.9" stroke-dasharray="2 2"
        />

        <For each={layout().cs}>{(c, i) => {
          const cx = padL + layout().xStep * (i() + 0.5);
          const up = c.close >= c.open;
          const vh = c.vol * volH;
          const yTop = padT + priceH + 4 + (volH - vh);
          return (
            <rect
              x={cx - layout().bodyW / 2} y={yTop}
              width={layout().bodyW} height={vh}
              fill={up ? "rgba(111,185,138,0.40)" : "rgba(217,112,112,0.40)"}
            />
          );
        }}</For>

        <For each={xLabels}>{(m, i) => (
          <text
            x={padL + (innerW * (i() / (xLabels.length - 1)))}
            y={H - 8}
            fill="rgba(168,158,140,0.6)" font-size="9" font-family="JetBrains Mono, monospace"
            text-anchor={i() === 0 ? "start" : (i() === xLabels.length - 1 ? "end" : "middle")}>
            {m}
          </text>
        )}</For>

        <g>
          <line x1={padL} x2={W - padR} y1={layout().yScale(layout().px)} y2={layout().yScale(layout().px)}
            stroke="rgba(217,119,87,0.55)" stroke-width="0.8" stroke-dasharray="3 3" />
          <rect x={W - padR + 1} y={layout().yScale(layout().px) - 7} width="44" height="14" rx="2"
            fill="rgba(217,119,87,0.18)" stroke="rgba(217,119,87,0.55)" />
          <text x={W - padR + 23} y={layout().yScale(layout().px) + 3.5}
            fill="#e89a7e" font-size="9.5" font-family="JetBrains Mono, monospace" text-anchor="middle">
            {layout().px.toFixed(2)}
          </text>
        </g>
      </svg>
    </div>
  );
}

function ModPdfPage(props: { doc: PdfDoc; snippets: PdfSnippet[] }) {
  return (
    <div class="lk-mod lk-mod-pdf">
      <div class="page">
        <h6>{props.doc.title}</h6>
        <p>{props.doc.intro}</p>
        <p>{props.doc.body1}</p>
        <Show when={props.doc.highlight}><p><span class="hl">{props.doc.highlight}</span></p></Show>
        <p>{props.doc.body2}</p>
        <p>{props.doc.body3}</p>
        <span class="lbl">{props.doc.lbl}</span>
      </div>
      <div class="extract">
        <For each={props.snippets}>{(s) => (
          <div>
            <div class="qt">"{s.before}<mark>{s.mark}</mark>{s.after}"</div>
            <div class="cite">— {s.cite}</div>
          </div>
        )}</For>
      </div>
    </div>
  );
}

function ModCites(props: { items: CiteItem[] }) {
  return (
    <div class="lk-mod-cites">
      <For each={props.items}>{(it) => (
        <div class="cite-row" data-tier={it.tier}>
          <span class="pin" />
          <div class="body">
            <div class="path">{it.path}</div>
            <div class="title">{it.title}</div>
            <Show when={it.quote}><div class="qt">"{it.quote}"</div></Show>
          </div>
        </div>
      )}</For>
    </div>
  );
}

function ModNews(props: { items: NewsItem[] }) {
  return (
    <div class="lk-mod-news">
      <For each={props.items}>{(it) => (
        <div class="item">
          <span class="ts">{it.ts}</span>
          <span class="head">
            <span class="src">[{it.src}]</span>
            {it.head}
          </span>
          <span class={"imp " + (it.imp === "high" ? "hi" : "")}>{it.imp.toUpperCase()}</span>
        </div>
      )}</For>
    </div>
  );
}

function ModRows(props: { rows: WatchRow[] }) {
  return (
    <div class="lk-mod lk-mod-rows">
      <For each={props.rows}>{(r) => {
        const seed = (r.sym || "").charCodeAt(0) || 7;
        const candles = genCandles(seed, 24, 80);
        let lo = Infinity, hi = -Infinity;
        for (const c of candles) { lo = Math.min(lo, c.lo); hi = Math.max(hi, c.hi); }
        const w = 60, h = 14;
        const path = candles.map((c, j) => {
          const x = (j / (candles.length - 1)) * w;
          const y = h - ((c.close - lo) / (hi - lo)) * h;
          return `${j === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
        }).join(" ");
        return (
          <div class="row">
            <span class="sym">{r.sym}</span>
            <span class="name">{r.name}</span>
            <svg class="spark" width={w} height={h}>
              <path d={path} fill="none" stroke={r.ch >= 0 ? "#6fb98a" : "#d97070"} stroke-width="1" />
            </svg>
            <span class="px">{r.px.toFixed(2)}</span>
            <span class={"ch " + (r.ch >= 0 ? "up" : "dn")}>{r.ch >= 0 ? "+" : ""}{r.ch.toFixed(2)}%</span>
          </div>
        );
      }}</For>
    </div>
  );
}

function ModCmpTable(props: { headers: string[]; rows: CmpRow[] }) {
  return (
    <div class="lk-mod lk-mod-cmp">
      <table>
        <thead><tr><For each={props.headers}>{(h) => <th>{h}</th>}</For></tr></thead>
        <tbody>
          <For each={props.rows}>{(r) => (
            <tr class={r.hl ? "hl" : ""}>
              <For each={r.cells}>{(c) => <td class={c.cls ?? ""}>{c.v}</td>}</For>
            </tr>
          )}</For>
        </tbody>
      </table>
    </div>
  );
}

function ModSubAgent(props: { data: SubAgentData }) {
  return (
    <div class="lk-mod lk-mod-sub">
      <div class="role">◆ {props.data.role}</div>
      <div class="task">{props.data.task}</div>
      <div class="meter"><i style={{ width: `${Math.round(props.data.progress * 100)}%` }} /></div>
      <div class="meta">step {props.data.step}/{props.data.total} · {props.data.tools} tool calls · {props.data.elapsed}</div>
    </div>
  );
}

function ModValuation(props: { steps: ValuationStep[] }) {
  return (
    <div class="lk-mod lk-mod-val">
      <div class="ladder">
        <For each={props.steps}>{(s) => (
          <div class={"step " + (s.tot ? "tot" : "")}>
            <span class="lbl">{s.k}</span>
            <span class="v">{s.v}</span>
          </div>
        )}</For>
      </div>
    </div>
  );
}

function ModDecision(props: { data: DecisionData }) {
  return (
    <div class="lk-mod lk-mod-dec">
      <div class="head">
        <span class={"verdict " + props.data.verdict.toLowerCase()}>{props.data.verdict}</span>
        <span class="sym">{props.data.sym}</span>
        <span class="conf">CONF · <b>{props.data.confidence.toFixed(2)}</b></span>
      </div>
      <div class="gist">{props.data.gist}</div>
      <div class="params">
        <For each={props.data.params}>{(p) => (
          <div class="p"><span class="k">{p.k}</span><span class="v">{p.v}</span></div>
        )}</For>
      </div>
      <div class="actions">
        <button class="btn primary">REVIEW &amp; SUBMIT</button>
        <button class="btn ghost">EDIT PARAMS</button>
        <button class="btn ghost">PIN ↗</button>
      </div>
    </div>
  );
}

/* ============ Panel chrome ============ */

const KIND_META: Record<PanelKind, { kind: string; dot: string }> = {
  "quote-card":   { kind: "QUOTE",         dot: "var(--n-quote)" },
  "fundamentals": { kind: "FUNDAMENTALS",  dot: "var(--n-fund)" },
  "chart":        { kind: "CHART_OHLC",    dot: "var(--n-chart)" },
  "evidence":     { kind: "PRIMARY_DOC",   dot: "var(--clay-soft)" },
  "corpus":       { kind: "CORPUS_REFS",   dot: "var(--n-corpus)" },
  "compare":      { kind: "COMPARE",       dot: "var(--n-fund)" },
  "news":         { kind: "NEWS_FEED",     dot: "var(--amber)" },
  "subagent":     { kind: "SUBAGENT",      dot: "var(--n-sub)" },
  "valuation":    { kind: "VALUATION",     dot: "var(--n-fund)" },
  "decision":     { kind: "DECISION",      dot: "var(--n-decision)" },
  "watch":        { kind: "WATCH",         dot: "var(--ink-1)" },
};

export interface PanelProps {
  kind: PanelKind;
  title: string;
  sub?: string;
  x: number; y: number; w: number; h: number;
  modules: Module[];
  state?: string;
  animDelay?: number;
  actions?: boolean;
}

export function Panel(props: PanelProps) {
  const meta = () => KIND_META[props.kind] ?? { kind: String(props.kind).toUpperCase(), dot: "var(--clay)" };
  const style = (): JSX.CSSProperties => ({
    left: `${props.x}px`,
    top: `${props.y}px`,
    width: `${props.w}px`,
    height: `${props.h}px`,
    "animation-delay": `${props.animDelay ?? 0}ms`,
  });
  return (
    <div class="lk-panel" data-state={props.state ?? "idle"} style={style()}>
      <div class="lk-panel-head">
        <span class="kind">
          <span class="dot" style={{ background: meta().dot, "box-shadow": `0 0 6px ${meta().dot}` }} />
          {meta().kind}
        </span>
        <span class="title">{props.title}</span>
        <Show when={props.sub}><span class="sub">{props.sub}</span></Show>
        <Show when={props.actions !== false}>
          <span class="actions">
            <button title="Pin"><Icon name="pin" class="ic-xs" /></button>
            <button title="Expand"><Icon name="expand" class="ic-xs" /></button>
            <button title="Close"><Icon name="close" class="ic-xs" /></button>
          </span>
        </Show>
      </div>
      <div class="lk-panel-body">
        <For each={props.modules}>{(m) => {
          if (m.kind === "quote") return <ModQuote data={m.data} />;
          if (m.kind === "kv") return <ModKV title={m.title} rows={m.rows} />;
          if (m.kind === "candles") return <ModCandles {...m} />;
          if (m.kind === "pdf") return <ModPdfPage doc={m.doc} snippets={m.snippets} />;
          if (m.kind === "cites") return <ModCites items={m.items} />;
          if (m.kind === "news") return <ModNews items={m.items} />;
          if (m.kind === "rows") return <ModRows rows={m.rows} />;
          if (m.kind === "cmp") return <ModCmpTable headers={m.headers} rows={m.rows} />;
          if (m.kind === "sub") return <ModSubAgent data={m.data} />;
          if (m.kind === "valuation") return <ModValuation steps={m.steps} />;
          if (m.kind === "decision") return <ModDecision data={m.data} />;
          return null;
        }}</For>
      </div>
    </div>
  );
}
