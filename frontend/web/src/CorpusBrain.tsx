// CorpusBrain — wiki graph + activation overlay (M2.1).
//
// Renders the full corpus wiki graph (fetched once from the gateway's
// `/api/v1/corpus/graph`) as an SVG, with a force-directed layout the
// component runs in-process at mount (no D3 / sigma dependency — corpus
// is bounded at ~128 nodes and the spec calls out hand-rolled SVG as the
// recommended path). The overlay surfaces *which* wiki pages the agent
// has actually used during this session — four ranked levels:
//
//   live        | a `corpus_search` / `corpus_read` is in-flight RIGHT NOW
//   turn        | used in the CURRENT turn (completed)
//   session     | used in an EARLIER turn of this session
//   historical  | used in a PRIOR session (persisted in sessionStorage)
//
// The current-turn / past-turn distinction is derived from the turnIds
// the parent's `applyEvent` has already seen: the active turn is the most
// recent one with any activity. There is no separate "current turn"
// event — it falls out of the data the workbench already keeps.

import {
  createMemo,
  createResource,
  createSignal,
  For,
  onCleanup,
  onMount,
  Show,
} from "solid-js";

import { activationLevelOf } from "./store";
import type { Activation, ActivationLevel, BrainGraph, BrainNode, BrainEdge } from "./types";

type CorpusBrainProps = {
  /** Activation snapshot the store maintains (live + turn references). */
  activation: () => Activation;
  /** Current session id — used for historical persistence keying. */
  sessionId: () => string | null;
};

// Visual tuning constants — pulled to the top so the spec's "节点大小
// 用 confidence 编码 / 颜色用 tier 编码" stays one grep away.
const NODE_R_BY_CONFIDENCE: Record<string, number> = {
  high: 7,
  medium: 5,
  low: 3.5,
};
const NODE_R_DEFAULT = 4.5;

const TIER_COLOR: Record<string, string> = {
  principles: "#60a5fa", // blue (matches --blue-soft)
  knowledge: "#34d399", // green (matches --ok)
};
const TIER_COLOR_DEFAULT = "#94a3b8";

const ACTIVATION_LEVELS: ActivationLevel[] = ["live", "turn", "session", "historical"];

// ── force-directed layout ────────────────────────────────────────────
// A minimal Fruchterman-Reingold-flavoured simulation: every node
// repels every other node, every edge spring-pulls its endpoints toward
// an ideal length. We run a fixed iteration budget at mount, then freeze
// the positions — the corpus is read-only so re-layout is wasted work.
// Edge weights stiffen springs proportionally (heavier edges pull
// tighter), which clusters thematically related nodes without the JSON
// having to carry layout hints.

type LayoutNode = { id: string; x: number; y: number; vx: number; vy: number; r: number };

/** Edge filter for layout AND render. Default `>= 3` keeps a 128-node /
 *  ~5k-edge corpus visually readable; weight-2 noise is suppressed by
 *  default (the user can pull the slider down to 2 to see everything). */
const DEFAULT_EDGE_THRESHOLD = 3;

function runLayout(nodes: BrainNode[], edges: BrainEdge[], width: number, height: number) {
  const N = nodes.length;
  if (N === 0) return new Map<string, { x: number; y: number; r: number }>();

  // Seed: small circle around the centre. A deterministic seed keeps
  // re-mounts visually stable (a Math.random() seed would jitter the
  // graph every time the user switched tabs).
  const cx = width / 2;
  const cy = height / 2;
  const seedR = Math.min(width, height) * 0.3;
  const layoutNodes: LayoutNode[] = nodes.map((n, i) => {
    const angle = (i / N) * Math.PI * 2;
    return {
      id: n.id,
      x: cx + seedR * Math.cos(angle),
      y: cy + seedR * Math.sin(angle),
      vx: 0,
      vy: 0,
      r: NODE_R_BY_CONFIDENCE[n.confidence] ?? NODE_R_DEFAULT,
    };
  });
  const idx = new Map<string, number>(layoutNodes.map((n, i) => [n.id, i]));

  // Standard FR parameters. The ideal edge length `k` is the diagonal
  // divided by sqrt(N) — gives the graph room without flying apart.
  const area = width * height;
  const k = 0.7 * Math.sqrt(area / Math.max(N, 1));
  const k2 = k * k;
  const iterations = 250;
  // Cooling schedule: max displacement per step shrinks linearly so the
  // simulation settles (otherwise it oscillates forever).
  const maxT = width * 0.05;

  // Pre-filter edges to the rendered subset — layout shouldn't be
  // dominated by weight-2 noise that the user won't even see.
  const renderedEdges = edges.filter((e) => e.weight >= DEFAULT_EDGE_THRESHOLD);

  for (let iter = 0; iter < iterations; iter++) {
    const t = maxT * (1 - iter / iterations);

    // Repulsive forces: every pair pushes apart inversely with distance.
    for (let i = 0; i < N; i++) {
      layoutNodes[i].vx = 0;
      layoutNodes[i].vy = 0;
    }
    for (let i = 0; i < N; i++) {
      for (let j = i + 1; j < N; j++) {
        const dx = layoutNodes[i].x - layoutNodes[j].x;
        const dy = layoutNodes[i].y - layoutNodes[j].y;
        const distSq = Math.max(dx * dx + dy * dy, 0.01);
        const dist = Math.sqrt(distSq);
        // F_repel = k² / dist, applied along the unit vector.
        const force = k2 / dist;
        const fx = (dx / dist) * force;
        const fy = (dy / dist) * force;
        layoutNodes[i].vx += fx;
        layoutNodes[i].vy += fy;
        layoutNodes[j].vx -= fx;
        layoutNodes[j].vy -= fy;
      }
    }

    // Attractive forces: each edge pulls its endpoints together,
    // scaled by weight so densely-tagged pairs cluster tightly.
    for (const e of renderedEdges) {
      const i = idx.get(e.source);
      const j = idx.get(e.target);
      if (i === undefined || j === undefined) continue;
      const dx = layoutNodes[i].x - layoutNodes[j].x;
      const dy = layoutNodes[i].y - layoutNodes[j].y;
      const dist = Math.max(Math.sqrt(dx * dx + dy * dy), 0.01);
      // F_attract = dist² / k, scaled by edge weight.
      const force = ((dist * dist) / k) * Math.min(e.weight / 3, 2.5);
      const fx = (dx / dist) * force;
      const fy = (dy / dist) * force;
      layoutNodes[i].vx -= fx;
      layoutNodes[i].vy -= fy;
      layoutNodes[j].vx += fx;
      layoutNodes[j].vy += fy;
    }

    // Apply velocity, clamped by the cooling temperature and the canvas
    // bounds. A node hitting the edge stops at the edge instead of
    // bouncing — gives the visualization a soft margin.
    for (const ln of layoutNodes) {
      const vmag = Math.sqrt(ln.vx * ln.vx + ln.vy * ln.vy);
      if (vmag > 0) {
        const scale = Math.min(vmag, t) / vmag;
        ln.x += ln.vx * scale;
        ln.y += ln.vy * scale;
      }
      const margin = ln.r + 10;
      ln.x = Math.max(margin, Math.min(width - margin, ln.x));
      ln.y = Math.max(margin, Math.min(height - margin, ln.y));
    }
  }

  const out = new Map<string, { x: number; y: number; r: number }>();
  for (const ln of layoutNodes) out.set(ln.id, { x: ln.x, y: ln.y, r: ln.r });
  return out;
}

// ── component ────────────────────────────────────────────────────────

const VIEW_WIDTH = 920;
const VIEW_HEIGHT = 640;

export function CorpusBrain(props: CorpusBrainProps) {
  // The graph is static for the gateway's lifetime — fetch once.
  const [graph] = createResource<BrainGraph>(async () => {
    const res = await fetch("/api/v1/corpus/graph");
    if (!res.ok) throw new Error(`graph fetch ${res.status}`);
    return (await res.json()) as BrainGraph;
  });

  // Edge weight floor — controls how dense the rendered edge mat is.
  // Default `3` keeps the dispatch's "100-500 edges" intent visually,
  // even when the actual corpus tagging is denser; user can dial back.
  const [edgeThreshold, setEdgeThreshold] = createSignal(DEFAULT_EDGE_THRESHOLD);

  // The selected node — clicking opens a side detail. Hover only flashes
  // a label; the detail panel is intentional, not on every mouseover.
  const [selected, setSelected] = createSignal<BrainNode | null>(null);
  const [hovered, setHovered] = createSignal<string | null>(null);

  // Cached layout — recomputed when the graph data changes, NOT when the
  // edge threshold changes (re-running layout on every slider tick is
  // jarring and unnecessary; threshold only filters render).
  const positions = createMemo(() => {
    const g = graph();
    if (!g) return new Map<string, { x: number; y: number; r: number }>();
    return runLayout(g.nodes, g.edges, VIEW_WIDTH, VIEW_HEIGHT);
  });

  // Visible edges — filtered by the threshold AND by membership in
  // positions (defensive: a node missing from layout would render at 0,0).
  const visibleEdges = createMemo<BrainEdge[]>(() => {
    const g = graph();
    if (!g) return [];
    const pos = positions();
    return g.edges.filter(
      (e) => e.weight >= edgeThreshold() && pos.has(e.source) && pos.has(e.target),
    );
  });

  // Map nodeId → activation level (live / turn / session / historical),
  // computed from the store's activation snapshot. Replaying history
  // (a past `corpus_*` event) lands as a `turn` reference on its turn id
  // — same code path, no special case for history vs live. Delegates to
  // `store.activationLevelOf` so the unit tests cover this contract.
  const levelFor = (nodeId: string): ActivationLevel | null =>
    activationLevelOf(props.activation(), nodeId);

  // Activation summary numbers shown above the legend.
  const activeCounts = createMemo(() => {
    const counts: Record<ActivationLevel, number> = {
      live: 0,
      turn: 0,
      session: 0,
      historical: 0,
    };
    const g = graph();
    if (!g) return counts;
    for (const n of g.nodes) {
      const l = levelFor(n.id);
      if (l) counts[l] += 1;
    }
    return counts;
  });

  // Persist historical activations per session so a returning user sees
  // their wiki history wash across the brain. The store seeds the
  // current-session historical set from this key on mount — see the
  // `setupHistorical` helper in store.ts.
  // We just observe `historical` here; the lifecycle is store-managed.

  // Optional: clear the selection on escape so the user can dismiss a
  // detail panel without clicking elsewhere.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") setSelected(null);
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));

  // Render helpers ------------------------------------------------------

  const nodeColor = (n: BrainNode, level: ActivationLevel | null) => {
    const base = TIER_COLOR[n.tier] ?? TIER_COLOR_DEFAULT;
    if (level === "live" || level === "turn") return base;
    if (level === "session") return base; // saturation handled by opacity
    if (level === "historical") return base;
    return base;
  };

  const nodeOpacity = (level: ActivationLevel | null) => {
    if (level === "live") return 1;
    if (level === "turn") return 1;
    if (level === "session") return 0.85;
    if (level === "historical") return 0.55;
    return 0.4;
  };

  const edgeOpacity = (level: ActivationLevel | null, weight: number) => {
    // Activated-endpoint edges pop; everyone else is faint.
    if (level === "live" || level === "turn") return 0.55;
    return Math.min(0.12 + weight * 0.02, 0.35);
  };

  // For pulsing live nodes, the CSS keyframe handles animation; we just
  // tag the circle with a class.

  return (
    <Show
      when={graph()}
      fallback={
        <Show
          when={!graph.error}
          fallback={
            <div class="brain-empty">
              加载知识图谱失败。<br />
              确认 gateway 已启动，并在 corpus root 中存在 wiki 文件。
            </div>
          }
        >
          <div class="brain-empty">正在加载 corpus brain…</div>
        </Show>
      }
    >
      {(g) => (
        <div class="brain-root">
          <header class="brain-toolbar">
            <span class="brain-stats">
              {g().stats.node_count} 节点 · {visibleEdges().length}/{g().stats.edge_count} 边
            </span>
            <div class="brain-slider">
              <label>
                边权重 ≥
                <input
                  type="range"
                  min="2"
                  max="8"
                  value={edgeThreshold()}
                  onInput={(e) => setEdgeThreshold(Number(e.currentTarget.value))}
                />
                <b>{edgeThreshold()}</b>
              </label>
            </div>
            <div class="brain-legend">
              <For each={ACTIVATION_LEVELS}>
                {(lv) => (
                  <span class={`brain-leg brain-leg-${lv}`}>
                    <span class="brain-leg-dot" />
                    {legendLabel(lv)}
                    <Show when={activeCounts()[lv] > 0}>
                      <span class="muted"> · {activeCounts()[lv]}</span>
                    </Show>
                  </span>
                )}
              </For>
            </div>
          </header>

          <div class="brain-canvas-wrap">
            <svg
              class="brain-svg"
              viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
              preserveAspectRatio="xMidYMid meet"
              role="img"
              aria-label="Corpus wiki graph"
            >
              {/* Edges first so nodes render on top. */}
              <g class="brain-edges">
                <For each={visibleEdges()}>
                  {(e) => {
                    const pos = positions();
                    const ps = pos.get(e.source);
                    const pt = pos.get(e.target);
                    if (!ps || !pt) return null;
                    const lvS = levelFor(e.source);
                    const lvT = levelFor(e.target);
                    const dominant: ActivationLevel | null =
                      lvS && lvT
                        ? (ACTIVATION_LEVELS.find((l) => l === lvS || l === lvT) ?? null)
                        : (lvS ?? lvT ?? null);
                    return (
                      <line
                        x1={ps.x}
                        y1={ps.y}
                        x2={pt.x}
                        y2={pt.y}
                        stroke="currentColor"
                        stroke-width={Math.min(0.4 + e.weight * 0.15, 1.6)}
                        opacity={edgeOpacity(dominant, e.weight)}
                        class={dominant ? `brain-edge-${dominant}` : ""}
                      />
                    );
                  }}
                </For>
              </g>

              {/* Nodes. Hover label is intentionally one render path —
                  the title text is appended outside the loop above for
                  the hovered node so it doesn't overlap dozens of
                  neighbour labels at once. */}
              <g class="brain-nodes">
                <For each={g().nodes}>
                  {(n) => {
                    const p = positions().get(n.id);
                    if (!p) return null;
                    const lv = levelFor(n.id);
                    return (
                      <g
                        classList={{
                          "brain-node": true,
                          [`brain-node-${lv ?? "idle"}`]: true,
                          "brain-node-selected": selected()?.id === n.id,
                        }}
                        transform={`translate(${p.x},${p.y})`}
                        onMouseEnter={() => setHovered(n.id)}
                        onMouseLeave={() => setHovered((h) => (h === n.id ? null : h))}
                        onClick={() => setSelected(n)}
                      >
                        <Show when={lv === "live"}>
                          <circle
                            class="brain-node-pulse"
                            r={p.r + 6}
                            fill="none"
                            stroke={nodeColor(n, lv)}
                            stroke-width="1.5"
                          />
                        </Show>
                        <circle
                          r={p.r}
                          fill={nodeColor(n, lv)}
                          opacity={nodeOpacity(lv)}
                          stroke={lv === "turn" ? "#fbbf24" : "transparent"}
                          stroke-width={lv === "turn" ? 1.8 : 0}
                        />
                      </g>
                    );
                  }}
                </For>
              </g>

              {/* Hover label — one node at a time so the canvas doesn't
                  drown in text. */}
              <Show when={hovered()}>
                {(id) => {
                  const node = g().nodes.find((n) => n.id === id());
                  const p = positions().get(id());
                  if (!node || !p) return null;
                  return (
                    <g class="brain-label" transform={`translate(${p.x},${p.y - p.r - 6})`}>
                      <text
                        text-anchor="middle"
                        font-size="13"
                        fill="var(--text)"
                        style="paint-order: stroke; stroke: var(--bg); stroke-width: 4; stroke-linejoin: round;"
                      >
                        {node.title}
                      </text>
                    </g>
                  );
                }}
              </Show>
            </svg>

            {/* Detail panel for the selected node. */}
            <Show when={selected()}>
              {(n) => (
                <aside class="brain-detail">
                  <header>
                    <h3>{n().title}</h3>
                    <button class="brain-detail-close" onClick={() => setSelected(null)}>
                      ×
                    </button>
                  </header>
                  <dl class="brain-detail-list">
                    <dt>tier</dt>
                    <dd>{n().tier}</dd>
                    <dt>type</dt>
                    <dd>{n().type || <span class="muted">—</span>}</dd>
                    <dt>confidence</dt>
                    <dd>{n().confidence || <span class="muted">—</span>}</dd>
                    <dt>tags</dt>
                    <dd>
                      <Show when={n().tags.length > 0} fallback={<span class="muted">—</span>}>
                        <span class="brain-tags">
                          <For each={n().tags}>
                            {(t) => <span class="brain-tag">{t}</span>}
                          </For>
                        </span>
                      </Show>
                    </dd>
                    <dt>id</dt>
                    <dd class="brain-id">{n().id}</dd>
                  </dl>
                  <Show when={levelFor(n().id)}>
                    {(lv) => (
                      <p class={`brain-status brain-status-${lv()}`}>
                        当前激活级别：<b>{legendLabel(lv())}</b>
                      </p>
                    )}
                  </Show>
                </aside>
              )}
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}

function legendLabel(lv: ActivationLevel): string {
  if (lv === "live") return "实时";
  if (lv === "turn") return "本回合";
  if (lv === "session") return "本会话";
  return "历史";
}
