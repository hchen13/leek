// CorpusBrain widget at top-right of canvas.
// Wraps the vanilla-canvas force-directed graph from /src/corpus-brain.js.
// Ported from prototype/leek-workbench.jsx → SolidJS.

import { createEffect, onCleanup } from "solid-js";
import type { Scene } from "../scenes";

interface BrainAPI { fire(ids: string[]): void; stats(): unknown; }
interface BrainGlobal { mount(host: HTMLElement): BrainAPI; NODES: unknown[]; EDGES: unknown[]; }

declare global {
  interface Window { LeekBrain?: BrainGlobal; }
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

  createEffect(() => {
    if (!host || !window.LeekBrain) return;
    if (!api) api = window.LeekBrain.mount(host);

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
  );
}
