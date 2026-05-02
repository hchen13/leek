// CorpusBrain widget at top-right of canvas.
// Wraps the vanilla-canvas force-directed graph from /src/corpus-brain.js.
// Ported from prototype/leek-workbench.jsx → SolidJS.

import { Show, createEffect, createSignal, onCleanup } from "solid-js";
import type { Scene } from "../scenes";

interface BrainAPI { fire(ids: string[]): void; stats(): unknown; }
interface NodeMeta { title: string; tier: string }
interface BrainGlobal {
  mount(
    host: HTMLElement,
    opts?: { graph?: unknown; onNodeClick?: (id: string, meta: NodeMeta) => void },
  ): BrainAPI;
  NODES: unknown[];
  EDGES: unknown[];
}

declare global {
  interface Window { LeekBrain?: BrainGlobal; }
}

interface CorpusDoc {
  id: string;
  title: string;
  tier: string;
  layer: string;
  tags: string[];
  body: string;
}

const meta = (s: Scene) =>
  s === "deep" || s === "thinking-shallow" ? "indexing · live"
  : s === "delivered" ? "reasoning · cached"
  : "ambient";

const stats = (s: Scene) =>
  s === "deep" ? [
    { k: "QUERY", v: "NVDA + capex" },
    { k: "ACTIVE", v: "12" },
    { k: "DEPTH", v: "3" },
  ]
  : s === "delivered" ? [
    { k: "REFS", v: "9" },
    { k: "CONF", v: "0.78" },
    { k: "DRIFT", v: "−0.04" },
  ]
  : [
    { k: "DOCS", v: "14,238" },
    { k: "VECS", v: "2.1M" },
    { k: "TIERS", v: "P·K" },
  ];

export function BrainWidget(props: { scene: Scene; fireIds?: string[] }) {
  let host!: HTMLDivElement;
  let api: BrainAPI | null = null;
  let mounting = false;

  const [openDoc, setOpenDoc] = createSignal<CorpusDoc | null>(null);
  const [docError, setDocError] = createSignal<string | null>(null);
  const [docLoading, setDocLoading] = createSignal(false);

  async function loadDoc(id: string, fallbackTitle: string) {
    setDocLoading(true);
    setDocError(null);
    setOpenDoc({ id, title: fallbackTitle, tier: "", layer: "", tags: [], body: "" });
    try {
      const r = await fetch(`/api/v1/corpus/doc?id=${encodeURIComponent(id)}`);
      if (!r.ok) {
        setDocError(`HTTP ${r.status}`);
        return;
      }
      const doc: CorpusDoc = await r.json();
      setOpenDoc(doc);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : "network error";
      setDocError(msg);
    } finally {
      setDocLoading(false);
    }
  }

  async function ensureMounted() {
    if (api || mounting || !host || !window.LeekBrain) return;
    mounting = true;
    let graph: unknown = undefined;
    try {
      const r = await fetch("/api/v1/corpus/graph");
      if (r.ok) graph = await r.json();
    } catch {
      // network failed — fall back to bundled fixture
    }
    if (!host || !window.LeekBrain) {
      mounting = false;
      return;
    }
    api = window.LeekBrain.mount(host, {
      graph,
      onNodeClick: (id, meta) => void loadDoc(id, meta.title),
    });
    mounting = false;
  }

  createEffect(() => {
    if (!openDoc()) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") { setOpenDoc(null); setDocError(null); }
    };
    document.addEventListener("keydown", handler);
    onCleanup(() => document.removeEventListener("keydown", handler));
  });

  createEffect(() => {
    if (!host || !window.LeekBrain) return;
    if (!api && !mounting) {
      void ensureMounted().then(() => {
        const ids = props.fireIds;
        if (ids && api) api.fire(ids);
      });
      return;
    }

    const ids = props.fireIds;
    if (!ids || !api) return;
    api.fire(ids);

    if (props.scene === "thinking-shallow" || props.scene === "deep") {
      let i = 0;
      const handle = window.setInterval(() => {
        if (!api) return;
        api.fire([ids[i % ids.length]]);
        i++;
      }, 1300);
      onCleanup(() => window.clearInterval(handle));
    }
  });

  return (
    <>
      <div class="lk-brain-widget" ref={host}>
        <div class="lk-brain-head">
          <span class="label"><b>CORPUS</b> · BRAIN</span>
          <span class="meta">{meta(props.scene)}</span>
        </div>
        <div class="lk-brain-legend">
          <span class="it"><span class="sw" style={{ background: "var(--c-prin-wiki)" }} />principles · wikis</span>
          <span class="it"><span class="sw" style={{ background: "var(--c-prin-src)" }} />principles · sources</span>
          <span class="it"><span class="sw" style={{ background: "var(--c-know-wiki)" }} />knowledge · wikis</span>
          <span class="it"><span class="sw" style={{ background: "var(--c-know-src)" }} />knowledge · sources</span>
        </div>
        <div class="lk-brain-stats">
          {stats(props.scene).map((s) => (
            <span class="row">
              <span class="k">{s.k}</span>
              <span class="v">{s.v}</span>
            </span>
          ))}
        </div>
      </div>
      <Show when={openDoc()}>
        {(doc) => (
          <CorpusDocModal
            doc={doc()}
            loading={docLoading()}
            error={docError()}
            onClose={() => { setOpenDoc(null); setDocError(null); }}
          />
        )}
      </Show>
    </>
  );
}

function CorpusDocModal(props: {
  doc: CorpusDoc;
  loading: boolean;
  error: string | null;
  onClose: () => void;
}) {
  return (
    <div
      onClick={props.onClose}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(8, 6, 4, 0.55)",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "z-index": 1000,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "min(820px, 92vw)",
          "max-height": "85vh",
          background: "var(--bg-1)",
          border: "1px solid var(--bg-2)",
          "border-radius": "12px",
          padding: "24px 28px",
          display: "flex",
          "flex-direction": "column",
          gap: "12px",
          color: "var(--ink-1)",
          "font-family": "var(--font-sans)",
        }}
      >
        <div style={{
          display: "flex",
          "align-items": "baseline",
          "justify-content": "space-between",
          gap: "16px",
        }}>
          <div>
            <div style={{ "font-size": "18px", "font-weight": 600 }}>
              {props.doc.title || props.doc.id}
            </div>
            <div style={{
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              color: "var(--ink-3)",
              "margin-top": "4px",
            }}>
              {props.doc.id}
              <Show when={props.doc.tier && props.doc.layer}>
                {" · "}{props.doc.tier} / {props.doc.layer}
              </Show>
            </div>
          </div>
          <button
            onClick={props.onClose}
            style={{
              background: "transparent",
              border: "1px solid var(--bg-2)",
              color: "var(--ink-2)",
              "border-radius": "6px",
              padding: "4px 12px",
              cursor: "pointer",
              "font-family": "var(--font-mono)",
              "font-size": "11px",
            }}
          >Esc · close</button>
        </div>
        <Show when={props.doc.tags.length > 0}>
          <div style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "6px",
            "font-family": "var(--font-mono)",
            "font-size": "10px",
            color: "var(--ink-3)",
          }}>
            {props.doc.tags.map((t) => (
              <span style={{
                padding: "2px 8px",
                background: "var(--bg-2)",
                "border-radius": "10px",
              }}>{t}</span>
            ))}
          </div>
        </Show>
        <div style={{
          flex: 1,
          overflow: "auto",
          "padding-right": "8px",
          "white-space": "pre-wrap",
          "font-size": "13px",
          "line-height": 1.6,
        }}>
          <Show
            when={!props.loading && !props.error}
            fallback={
              <div style={{ color: "var(--ink-3)", "font-style": "italic" }}>
                {props.loading ? "loading…" : `error: ${props.error}`}
              </div>
            }
          >
            {props.doc.body}
          </Show>
        </div>
      </div>
    </div>
  );
}
