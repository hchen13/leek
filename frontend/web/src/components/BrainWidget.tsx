// CorpusBrain widget at top-right of canvas.
// Wraps the vanilla-canvas force-directed graph from /src/corpus-brain.js.
// Ported from prototype/leek-workbench.jsx → SolidJS.

import { createEffect, onCleanup } from "solid-js";
import type { Scene } from "../scenes";

interface BrainAPI {
  fire(ids: string[]): void;
  setActivated(ids: string[]): void;
  recenter(): void;
  stats(): unknown;
}
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

const meta = (s: Scene) =>
  s === "deep" || s === "thinking-shallow" ? "indexing · live"
  : s === "delivered" ? "reasoning · cached"
  : "ambient";

export function BrainWidget(props: {
  scene: Scene;
  fireIds?: string[];
  activatedIds?: string[];
  onOpenDoc?: (id: string, title: string) => void;
}) {
  let host!: HTMLDivElement;
  let api: BrainAPI | null = null;
  let mounting = false;

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
      onNodeClick: (id, meta) => props.onOpenDoc?.(id, meta.title),
    });
    mounting = false;
  }

  createEffect(() => {
    if (!host || !window.LeekBrain) return;
    if (!api && !mounting) {
      void ensureMounted().then(() => {
        const ids = props.fireIds;
        if (ids && api) api.fire(ids);
      });
      return;
    }

    // Push session-scoped activation set whenever it changes. Empty array
    // resets the brain to "all dimly lit" (idle / fresh session).
    if (api && props.activatedIds) {
      api.setActivated(props.activatedIds);
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
    <div class="lk-brain-widget" ref={host}>
      <div class="lk-brain-head">
        <span class="label"><b>CORPUS</b> · BRAIN</span>
        <span class="meta">{meta(props.scene)}</span>
      </div>
    </div>
  );
}
