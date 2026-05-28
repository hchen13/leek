// CorpusMinimap — the 256×256 wiki graph (DESIGN.md §5.7).
//
// A force-laid SVG, but nodes are tiny dots (3-4px) so the whole graph
// reads as a "what part of the corpus is active right now" minimap.
// Activation overlay maps to design tokens:
//   - live (this turn, in flight)   → `var(--accent)` + lk-pulse animation
//   - turn (this turn, completed)   → `var(--accent)` solid, no pulse
//   - session (earlier turn)        → `var(--accent)` at 0.5 opacity
//   - historical (prior session)    → `var(--muted)`
//   - idle                          → `var(--meta)` at 0.3 opacity
//
// Click → fires `onFullscreen` so the parent can pop up the big graph
// (CorpusBrain.tsx, retained for fullscreen use).
//
// State coverage:
//   - graph loading    → "加载中…" pulse skeleton
//   - graph load error → muted "未能加载" placeholder
//   - empty corpus     → dimmed placeholder "尚未生成 corpus"
//   - normal           → dotted force layout

import { createMemo, createResource, Show } from "solid-js";

import { activationLevelOf } from "../store";
import type { Activation, ActivationLevel, BrainGraph, BrainEdge, BrainNode } from "../types";

type Props = {
  activation: () => Activation;
  /** Click handler — parent toggles a `lk-corpus-fullscreen` overlay. */
  onFullscreen: () => void;
};

const VIEW = 256; // matches DESIGN.md --corpus-map-size

/** Compact FR-like force layout. Same algorithm as CorpusBrain but
 *  scaled down — fewer iterations, smaller nodes, no labels. Frozen
 *  after layout, since the corpus is read-only. */
type LayoutNode = { id: string; x: number; y: number; vx: number; vy: number };

function runLayout(nodes: BrainNode[], edges: BrainEdge[], size: number) {
  const N = nodes.length;
  const out = new Map<string, { x: number; y: number }>();
  if (N === 0) return out;
  const cx = size / 2;
  const cy = size / 2;
  const seedR = size * 0.32;
  const layout: LayoutNode[] = nodes.map((n, i) => ({
    id: n.id,
    x: cx + seedR * Math.cos((i / N) * Math.PI * 2),
    y: cy + seedR * Math.sin((i / N) * Math.PI * 2),
    vx: 0,
    vy: 0,
  }));
  const idx = new Map(layout.map((l, i) => [l.id, i]));
  // Tighter k for the smaller canvas; fewer iters.
  const k = 0.7 * Math.sqrt((size * size) / Math.max(N, 1));
  const k2 = k * k;
  const iters = 150;
  const maxT = size * 0.06;
  const edgeSet = edges.filter((e) => e.weight >= 3);

  for (let it = 0; it < iters; it++) {
    const t = maxT * (1 - it / iters);
    for (const ln of layout) {
      ln.vx = 0;
      ln.vy = 0;
    }
    for (let i = 0; i < N; i++) {
      for (let j = i + 1; j < N; j++) {
        const dx = layout[i].x - layout[j].x;
        const dy = layout[i].y - layout[j].y;
        const dist = Math.sqrt(Math.max(dx * dx + dy * dy, 0.01));
        const force = k2 / dist;
        const fx = (dx / dist) * force;
        const fy = (dy / dist) * force;
        layout[i].vx += fx;
        layout[i].vy += fy;
        layout[j].vx -= fx;
        layout[j].vy -= fy;
      }
    }
    for (const e of edgeSet) {
      const i = idx.get(e.source);
      const j = idx.get(e.target);
      if (i == null || j == null) continue;
      const dx = layout[i].x - layout[j].x;
      const dy = layout[i].y - layout[j].y;
      const dist = Math.max(Math.sqrt(dx * dx + dy * dy), 0.01);
      const force = ((dist * dist) / k) * Math.min(e.weight / 3, 2.5);
      const fx = (dx / dist) * force;
      const fy = (dy / dist) * force;
      layout[i].vx -= fx;
      layout[i].vy -= fy;
      layout[j].vx += fx;
      layout[j].vy += fy;
    }
    for (const ln of layout) {
      const vmag = Math.sqrt(ln.vx * ln.vx + ln.vy * ln.vy);
      if (vmag > 0) {
        const scale = Math.min(vmag, t) / vmag;
        ln.x += ln.vx * scale;
        ln.y += ln.vy * scale;
      }
      const margin = 8;
      ln.x = Math.max(margin, Math.min(size - margin, ln.x));
      ln.y = Math.max(margin, Math.min(size - margin, ln.y));
    }
  }
  for (const ln of layout) out.set(ln.id, { x: ln.x, y: ln.y });
  return out;
}

function nodeClassFor(level: ActivationLevel | null): string {
  switch (level) {
    case "live":
      return "lk-mini-node lk-mini-node--live";
    case "turn":
      return "lk-mini-node lk-mini-node--turn";
    case "session":
      return "lk-mini-node lk-mini-node--session";
    case "historical":
      return "lk-mini-node lk-mini-node--historical";
    default:
      return "lk-mini-node lk-mini-node--idle";
  }
}

export function CorpusMinimap(props: Props) {
  const [graph] = createResource<BrainGraph>(async () => {
    const res = await fetch("/api/v1/corpus/graph");
    if (!res.ok) throw new Error(`graph fetch ${res.status}`);
    return (await res.json()) as BrainGraph;
  });

  const positions = createMemo(() => {
    const g = graph();
    if (!g) return new Map<string, { x: number; y: number }>();
    return runLayout(g.nodes, g.edges, VIEW);
  });

  const visibleEdges = createMemo<BrainEdge[]>(() => {
    const g = graph();
    if (!g) return [];
    const pos = positions();
    return g.edges.filter(
      (e) => e.weight >= 3 && pos.has(e.source) && pos.has(e.target),
    );
  });

  const levelFor = (nodeId: string): ActivationLevel | null =>
    activationLevelOf(props.activation(), nodeId);

  return (
    <button
      type="button"
      class="lk-minimap"
      onClick={() => props.onFullscreen()}
      title="点击展开 corpus 全图"
      aria-label="点击展开 corpus 全图"
    >
      <Show
        when={graph()}
        fallback={
          <Show
            when={!graph.error}
            fallback={
              <div class="lk-minimap-state lk-minimap-state--error">
                未能加载知识图谱
              </div>
            }
          >
            <div class="lk-minimap-state lk-minimap-state--loading">
              <span class="lk-pulse lk-mini-loading-dot" aria-hidden="true" />
              <span>加载中…</span>
            </div>
          </Show>
        }
      >
        {(g) => (
          <Show
            when={g().nodes.length > 0}
            fallback={
              <div class="lk-minimap-state lk-minimap-state--empty">
                尚未生成 corpus
              </div>
            }
          >
            <svg
              class="lk-minimap-svg"
              viewBox={`0 0 ${VIEW} ${VIEW}`}
              preserveAspectRatio="xMidYMid meet"
              role="img"
              aria-label="corpus 知识图谱缩略"
            >
              <g class="lk-mini-edges">
                {visibleEdges().map((e) => {
                  const ps = positions().get(e.source);
                  const pt = positions().get(e.target);
                  if (!ps || !pt) return null;
                  return (
                    <line
                      x1={ps.x}
                      y1={ps.y}
                      x2={pt.x}
                      y2={pt.y}
                      class="lk-mini-edge"
                    />
                  );
                })}
              </g>
              <g class="lk-mini-nodes">
                {g().nodes.map((n) => {
                  const p = positions().get(n.id);
                  if (!p) return null;
                  const lv = levelFor(n.id);
                  return (
                    <g class={nodeClassFor(lv)} transform={`translate(${p.x},${p.y})`}>
                      <Show when={lv === "live"}>
                        <circle r={6} class="lk-mini-node-ring" />
                      </Show>
                      <circle r={lv === "live" || lv === "turn" ? 3.5 : 2.5} />
                    </g>
                  );
                })}
              </g>
            </svg>
          </Show>
        )}
      </Show>
    </button>
  );
}
