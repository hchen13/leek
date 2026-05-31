import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import type { Scene } from "../scenes";

interface CorpusToolCall {
  name: string;
  status: string;
  arguments?: string;
  output_preview?: string;
}

interface CorpusHit {
  id: string;
  title: string;
  tier: string;
  layer: string;
  score?: number;
  usage_role?: string;
}

interface CorpusTraceEntry extends CorpusHit {
  query: string;
}

interface CorpusGraphNode {
  id: string;
  cluster: string;
  title: string;
  slug: string;
  type?: string;
  tier: string;
  layer: string;
  tags?: string[];
  degree: number;
}

interface CorpusGraphEdge {
  from: string;
  to: string;
  kind: string;
}

interface CorpusGraph {
  version: number;
  generated_at: string;
  nodes: CorpusGraphNode[];
  edges: CorpusGraphEdge[];
}

interface LayoutNode extends CorpusGraphNode {
  x: number;
  y: number;
  r: number;
  color: string;
}

interface ForceNode extends LayoutNode {
  vx: number;
  vy: number;
}

interface LayoutEdge extends CorpusGraphEdge {
  a: LayoutNode;
  b: LayoutNode;
}

interface CorpusLayout {
  nodes: LayoutNode[];
  edges: LayoutEdge[];
  byId: Map<string, LayoutNode>;
  adjacent: Map<string, string[]>;
}

interface BrainViewport {
  panX: number;
  panY: number;
  zoom: number;
}

export interface AgentPlanItem {
  id?: string;
  seq?: number;
  step: string;
  status: "pending" | "in_progress" | "completed" | string;
  evidence?: string | null;
}

export interface AgentPlanView {
  task_id?: string | null;
  explanation?: string | null;
  items: AgentPlanItem[];
}

const sceneMeta = (s: Scene) =>
  s === "deep" || s === "thinking-shallow" ? "检索中 · 实时"
  : s === "delivered" ? "检索完成"
  : "待命";

const CLUSTERS: Record<string, { label: string; color: string; cx: number; cy: number }> = {
  "principles-wikis": { label: "原则 · 概念", color: "#d97757", cx: 0.25, cy: 0.29 },
  "principles-sources": { label: "原则 · 源材料", color: "#a85636", cx: 0.27, cy: 0.73 },
  "knowledge-wikis": { label: "知识 · 概念", color: "#e8b86c", cx: 0.76, cy: 0.31 },
  "knowledge-sources": { label: "知识 · 源材料", color: "#b88842", cx: 0.74, cy: 0.74 },
};

const BOOTSTRAP_ACTIVE_IDS = [
  "wikis/principles/concepts/principles-runtime-kernel",
  "wikis/principles/entities/warren-buffett",
  "wikis/principles/entities/charlie-munger",
  "wikis/principles/entities/ray-dalio",
];

const DEFAULT_BRAIN_VIEWPORT: BrainViewport = { panX: 0, panY: 0, zoom: 1.06 };
const BRAIN_PHYSICS = {
  center: 0.5,
  repulsion: 10,
  attraction: 1,
  linkLength: 345,
};

function safeParseJson(s: string | undefined): unknown {
  if (!s) return null;
  try { return JSON.parse(s); } catch { return null; }
}

function parseArgs(t: CorpusToolCall): Record<string, unknown> {
  if (!t.arguments) return {};
  try { return JSON.parse(t.arguments); } catch { return {}; }
}

function extractObjectsAfter(s: string, key: string): unknown[] {
  const keyIdx = s.indexOf(key);
  if (keyIdx < 0) return [];
  const arrIdx = s.indexOf("[", keyIdx);
  if (arrIdx < 0) return [];
  const out: unknown[] = [];
  let i = arrIdx + 1;
  while (i < s.length) {
    while (i < s.length && /[\s,]/.test(s[i])) i++;
    if (s[i] === "]") break;
    if (s[i] !== "{") { i++; continue; }
    const start = i;
    let depth = 0;
    let inString = false;
    let escaped = false;
    let end = -1;
    for (; i < s.length; i++) {
      const ch = s[i];
      if (inString) {
        if (escaped) escaped = false;
        else if (ch === "\\") escaped = true;
        else if (ch === "\"") inString = false;
        continue;
      }
      if (ch === "\"") inString = true;
      else if (ch === "{") depth++;
      else if (ch === "}") {
        depth--;
        if (depth === 0) { end = i; i++; break; }
      }
    }
    if (end < 0) break;
    try { out.push(JSON.parse(s.slice(start, end + 1))); } catch {}
  }
  return out;
}

function parseCorpusHits(tool: CorpusToolCall): CorpusTraceEntry[] {
  const args = parseArgs(tool);
  const query = String(args.query ?? "");
  const preview = tool.output_preview ?? "";
  const parsed = safeParseJson(preview) as { hits?: CorpusHit[] } | null;
  const hits = parsed?.hits ?? extractObjectsAfter(preview, "\"hits\"") as CorpusHit[];
  return hits
    .filter((h) => h?.id && h?.title && h?.tier && h?.layer)
    .map((h) => ({ ...h, query }));
}

function uniqueEntries(entries: CorpusTraceEntry[]) {
  const seen = new Set<string>();
  return entries.filter((entry) => {
    if (seen.has(entry.id)) return false;
    seen.add(entry.id);
    return true;
  });
}

function hashString(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function clamp(n: number, min: number, max: number) {
  return Math.max(min, Math.min(max, n));
}

function clusterFor(cluster: string) {
  return CLUSTERS[cluster] ?? { label: cluster, color: "#a89e8c", cx: 0.5, cy: 0.5 };
}

function linkTarget(edge: CorpusGraphEdge, a: LayoutNode, b: LayoutNode) {
  const lengthNorm = (BRAIN_PHYSICS.linkLength - 30) / (500 - 30);
  const base = 0.075 + lengthNorm * 0.245;
  if (edge.kind === "same-source") return base * 0.72;
  if (a.cluster === b.cluster) return base * (a.layer === "wiki" || b.layer === "wiki" ? 0.82 : 0.9);
  if (a.layer === b.layer) return base * 1.08;
  return base * 1.18;
}

function runForceLayout(nodes: ForceNode[], rawEdges: CorpusGraphEdge[], byId: Map<string, ForceNode>) {
  const edges = rawEdges
    .map((edge) => {
      const a = byId.get(edge.from);
      const b = byId.get(edge.to);
      return a && b && a.id !== b.id ? { edge, a, b, target: linkTarget(edge, a, b) } : null;
    })
    .filter(Boolean) as Array<{ edge: CorpusGraphEdge; a: ForceNode; b: ForceNode; target: number }>;

  const centerStrength = 0.004 + BRAIN_PHYSICS.center * 0.01;
  const clusterStrength = 0.00035 + BRAIN_PHYSICS.center * 0.00045;
  const repulsionNorm = BRAIN_PHYSICS.repulsion / 20;
  const repulsionRadius = 0.2 + repulsionNorm * 0.22;
  const chargeBase = 0.000012 + repulsionNorm * 0.000055;
  const attractionStrength = 0.012 + BRAIN_PHYSICS.attraction * 0.018;

  for (let iter = 0; iter < 360; iter++) {
    const heat = 1 - iter / 360;
    for (let i = 0; i < nodes.length; i++) {
      for (let j = i + 1; j < nodes.length; j++) {
        const a = nodes[i];
        const b = nodes[j];
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const distSq = dx * dx + dy * dy + 0.0004;
        const dist = Math.sqrt(distSq);
        if (dist > repulsionRadius) continue;
        const falloff = 1 - dist / repulsionRadius;
        const charge = chargeBase * (a.layer === "wiki" || b.layer === "wiki" ? 1.18 : 1) / distSq * falloff;
        const fx = dx / dist * charge;
        const fy = dy / dist * charge;
        a.vx -= fx;
        a.vy -= fy;
        b.vx += fx;
        b.vy += fy;
      }
    }

    for (const node of nodes) {
      const cfg = clusterFor(node.cluster);
      node.vx += (0.5 - node.x) * centerStrength;
      node.vy += (0.5 - node.y) * centerStrength;
      node.vx += (cfg.cx - node.x) * clusterStrength;
      node.vy += (cfg.cy - node.y) * clusterStrength;
    }

    for (const { a, b, target } of edges) {
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const dist = Math.sqrt(dx * dx + dy * dy) || 0.0001;
      const pull = (dist - target) * attractionStrength;
      const fx = dx / dist * pull;
      const fy = dy / dist * pull;
      a.vx += fx;
      a.vy += fy;
      b.vx -= fx;
      b.vy -= fy;
    }

    for (const node of nodes) {
      const speedLimit = 0.024 * Math.max(0.35, heat);
      node.vx = clamp(node.vx * 0.78, -speedLimit, speedLimit);
      node.vy = clamp(node.vy * 0.78, -speedLimit, speedLimit);
      node.x += node.vx;
      node.y += node.vy;
      if (node.x < -0.16) { node.x = -0.16; node.vx *= -0.25; }
      if (node.x > 1.16) { node.x = 1.16; node.vx *= -0.25; }
      if (node.y < -0.14) { node.y = -0.14; node.vy *= -0.25; }
      if (node.y > 1.14) { node.y = 1.14; node.vy *= -0.25; }
    }
  }
}

function buildLayout(graph: CorpusGraph): CorpusLayout {
  const grouped = new Map<string, CorpusGraphNode[]>();
  for (const node of graph.nodes) {
    const list = grouped.get(node.cluster) ?? [];
    list.push(node);
    grouped.set(node.cluster, list);
  }

  const nodes: ForceNode[] = [];
  for (const [cluster, list] of grouped) {
    const cfg = clusterFor(cluster);
    const sorted = [...list].sort((a, b) => b.degree - a.degree || a.title.localeCompare(b.title));
    const maxDegree = Math.max(1, ...sorted.map((n) => n.degree));
    sorted.forEach((node, i) => {
      const h = hashString(node.id);
      const angle = i * 2.399963 + (h % 360) * Math.PI / 720;
      const rank = sorted.length <= 1 ? 0 : i / (sorted.length - 1);
      const spread = 0.045 + Math.sqrt(rank) * 0.20;
      const degreePull = Math.sqrt(node.degree / maxDegree) * 0.045;
      const jitterX = ((h >>> 8) % 1000) / 1000 - 0.5;
      const jitterY = ((h >>> 18) % 1000) / 1000 - 0.5;
      const radius = Math.max(0.016, spread - degreePull);
      const x = clamp(cfg.cx + Math.cos(angle) * radius + jitterX * 0.035, 0.055, 0.945);
      const y = clamp(cfg.cy + Math.sin(angle) * radius * 0.88 + jitterY * 0.03, 0.07, 0.93);
      const r = 0.75 + Math.min(2.1, Math.sqrt(node.degree + 1) * 0.145) + (node.layer === "wiki" ? 0.22 : 0);
      nodes.push({ ...node, x, y, r, color: cfg.color, vx: 0, vy: 0 });
    });
  }

  const byId = new Map(nodes.map((node) => [node.id, node]));
  runForceLayout(nodes, graph.edges, byId);

  const edges: LayoutEdge[] = [];
  const adjacent = new Map<string, string[]>();
  const seen = new Set<string>();
  for (const edge of graph.edges) {
    const a = byId.get(edge.from);
    const b = byId.get(edge.to);
    if (!a || !b || a.id === b.id) continue;
    const key = edge.from < edge.to ? `${edge.from}|${edge.to}` : `${edge.to}|${edge.from}`;
    if (seen.has(key)) continue;
    seen.add(key);
    edges.push({ ...edge, a, b });
    adjacent.set(edge.from, [...(adjacent.get(edge.from) ?? []), edge.to]);
    adjacent.set(edge.to, [...(adjacent.get(edge.to) ?? []), edge.from]);
  }

  return { nodes, edges, byId, adjacent };
}

function colorToRgb(hex: string) {
  const raw = hex.replace("#", "");
  const n = parseInt(raw.length === 3 ? raw.split("").map((c) => c + c).join("") : raw, 16);
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 };
}

function rgba(hex: string, alpha: number) {
  const c = colorToRgb(hex);
  return `rgba(${c.r}, ${c.g}, ${c.b}, ${alpha})`;
}

function toScreen(node: LayoutNode, w: number, h: number, viewport: BrainViewport) {
  const worldW = w * viewport.zoom;
  const worldH = h * viewport.zoom;
  return {
    x: w / 2 + viewport.panX + (node.x - 0.5) * worldW,
    y: h / 2 + viewport.panY + (node.y - 0.5) * worldH,
  };
}

function drawBrain(
  canvas: HTMLCanvasElement,
  layout: CorpusLayout,
  active: Map<string, number>,
  exactActive: Set<string>,
  hovered: string | null,
  ts: number,
  viewport: BrainViewport,
) {
  const rect = canvas.getBoundingClientRect();
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const width = Math.max(1, Math.floor(rect.width));
  const height = Math.max(1, Math.floor(rect.height));
  const pixelWidth = Math.floor(width * dpr);
  const pixelHeight = Math.floor(height * dpr);
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }

  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);

  const glow = ctx.createRadialGradient(width * 0.5, height * 0.48, 10, width * 0.5, height * 0.48, Math.max(width, height) * 0.62);
  glow.addColorStop(0, "rgba(232, 154, 126, 0.075)");
  glow.addColorStop(0.52, "rgba(232, 184, 108, 0.025)");
  glow.addColorStop(1, "rgba(0, 0, 0, 0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, width, height);

  ctx.lineWidth = 0.65;
  for (const edge of layout.edges) {
    const a = toScreen(edge.a, width, height, viewport);
    const b = toScreen(edge.b, width, height, viewport);
    const directlyLit = exactActive.has(edge.from) && exactActive.has(edge.to);
    const hoveredEdge = hovered ? edge.from === hovered || edge.to === hovered : false;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y);
    ctx.lineTo(b.x, b.y);
    ctx.strokeStyle = directlyLit
      ? "rgba(232, 154, 126, 0.2)"
      : hoveredEdge
      ? "rgba(255, 240, 220, 0.055)"
      : "rgba(255, 240, 220, 0.018)";
    ctx.stroke();
  }

  for (const node of layout.nodes) {
    const p = toScreen(node, width, height, viewport);
    const level = active.get(node.id) ?? 0;
    const isHovered = hovered === node.id;
    const isExact = exactActive.has(node.id);
    const pulse = (Math.sin(ts / 850 + (hashString(node.id) % 80) / 10) + 1) / 2;

    if (level > 0 || isHovered) {
      ctx.beginPath();
      ctx.arc(p.x, p.y, node.r + (isExact ? 2.8 : 1.8) + pulse * (isExact ? 1.4 : 1), 0, Math.PI * 2);
      ctx.fillStyle = rgba(node.color, isExact ? 0.09 + pulse * 0.07 : 0.045 + level * 0.018 + pulse * 0.025);
      ctx.fill();
    }

    ctx.beginPath();
    ctx.arc(p.x, p.y, node.r + (isExact ? 1 + pulse * 0.45 : level > 0 ? 0.55 + pulse * 0.25 : 0) + (isHovered ? 1.1 : 0), 0, Math.PI * 2);
    ctx.fillStyle = level > 0 || isHovered
      ? rgba(node.color, 0.9)
      : rgba(node.color, node.layer === "wiki" ? 0.15 : 0.075);
    ctx.fill();

    ctx.beginPath();
    ctx.arc(p.x, p.y, node.r + (isExact ? 1.8 + pulse * 0.5 : 0.65), 0, Math.PI * 2);
    ctx.strokeStyle = isExact
      ? "rgba(246, 239, 226, 0.72)"
      : level > 1 || isHovered
      ? "rgba(246, 239, 226, 0.55)"
      : "rgba(255, 240, 220, 0.045)";
    ctx.stroke();

    if (isHovered) {
      ctx.font = "10px Inter, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      const label = node.title.length > 28 ? `${node.title.slice(0, 27)}…` : node.title;
      const tw = ctx.measureText(label).width;
      const lx = clamp(p.x + node.r + 6, 6, width - tw - 10);
      const ly = clamp(p.y, 10, height - 10);
      ctx.fillStyle = "rgba(26, 23, 20, 0.72)";
      ctx.fillRect(lx - 4, ly - 8, tw + 8, 16);
      ctx.fillStyle = "rgba(246, 239, 226, 0.9)";
      ctx.fillText(label, lx, ly);
    }
  }
}

function hitNode(layout: CorpusLayout, canvas: HTMLCanvasElement, x: number, y: number, viewport: BrainViewport) {
  const rect = canvas.getBoundingClientRect();
  let best: LayoutNode | null = null;
  let bestDist = Infinity;
  for (const node of layout.nodes) {
    const p = toScreen(node, rect.width, rect.height, viewport);
    const dist = Math.hypot(p.x - x, p.y - y);
    if (dist <= Math.max(8, node.r + 5) && dist < bestDist) {
      best = node;
      bestDist = dist;
    }
  }
  return best;
}

function CorpusGraphMap(props: {
  scene: Scene;
  entries: CorpusTraceEntry[];
  onOpenDoc?: (id: string, title: string) => void;
}) {
  const [graph, setGraph] = createSignal<CorpusGraph | null>(null);
  const [loadError, setLoadError] = createSignal<string | null>(null);
  const [hovered, setHovered] = createSignal<string | null>(null);
  const [viewport, setViewport] = createSignal<BrainViewport>(DEFAULT_BRAIN_VIEWPORT);
  const [dragging, setDragging] = createSignal(false);
  const [sizeTick, setSizeTick] = createSignal(0);
  const [visible, setVisible] = createSignal(true);
  const [documentVisible, setDocumentVisible] = createSignal(document.visibilityState === "visible");
  let canvas: HTMLCanvasElement | undefined;
  let drag:
    | { pointerId: number; startX: number; startY: number; panX: number; panY: number; moved: boolean }
    | null = null;
  let suppressClick = false;

  onMount(() => {
    let cancelled = false;
    void fetch("/api/v1/corpus/graph")
      .then((r) => r.ok ? r.json() : Promise.reject(new Error(`HTTP ${r.status}`)))
      .then((g: CorpusGraph) => { if (!cancelled) setGraph(g); })
      .catch((e: Error) => { if (!cancelled) setLoadError(e.message); });

    const ro = new ResizeObserver(() => setSizeTick((v) => v + 1));
    if (canvas?.parentElement) ro.observe(canvas.parentElement);
    const io = new IntersectionObserver((entries) => {
      setVisible(entries.some((entry) => entry.isIntersecting));
    }, { threshold: 0.05 });
    if (canvas?.parentElement) io.observe(canvas.parentElement);
    const onVisibilityChange = () => setDocumentVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", onVisibilityChange);
    onCleanup(() => {
      cancelled = true;
      ro.disconnect();
      io.disconnect();
      document.removeEventListener("visibilitychange", onVisibilityChange);
    });
  });

  const layout = createMemo(() => {
    const g = graph();
    return g ? buildLayout(g) : null;
  });

  const exactActive = createMemo(() => {
    const out = new Set(BOOTSTRAP_ACTIVE_IDS);
    for (const entry of props.entries) out.add(entry.id);
    return out;
  });

  const active = createMemo(() => {
    const l = layout();
    const out = new Map<string, number>();
    if (l) {
      for (const id of BOOTSTRAP_ACTIVE_IDS) {
        if (l.byId.has(id)) out.set(id, 3);
      }
    }
    for (const entry of props.entries) {
      out.set(entry.id, entry.layer === "wiki" ? 3 : 2);
    }
    if (hovered()) out.set(hovered()!, Math.max(out.get(hovered()!) ?? 0, 2));
    return out;
  });

  createEffect(() => {
    const l = layout();
    const c = canvas;
    const a = active();
    const exact = exactActive();
    const h = hovered();
    const v = viewport();
    const isVisible = visible() && documentVisible();
    sizeTick();
    if (!l || !c || !isVisible) return;

    let raf = 0;
    let stopped = false;
    const animated = exact.size > 0;
    const frame = (ts: number) => {
      if (stopped) return;
      drawBrain(c, l, a, exact, h, ts, v);
      if (animated) raf = window.setTimeout(() => requestAnimationFrame(frame), 420);
    };
    requestAnimationFrame(frame);
    onCleanup(() => {
      stopped = true;
      window.clearTimeout(raf);
    });
  });

  const counts = createMemo(() => {
    const g = graph();
    if (!g) return null;
    return {
      nodes: g.nodes.length,
      edges: g.edges.length,
    };
  });

  const pointerPosition = (e: MouseEvent | PointerEvent) => {
    const rect = canvas!.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };

  const updateHover = (e: PointerEvent) => {
    const l = layout();
    if (!l || !canvas) return;
    const p = pointerPosition(e);
    const node = hitNode(l, canvas, p.x, p.y, viewport());
    setHovered(node?.id ?? null);
    canvas.style.cursor = node ? "pointer" : "grab";
  };

  return (
    <div class="lk-brain-map-wrap">
      <canvas
        ref={(el) => (canvas = el)}
        class="lk-brain-map"
        onPointerDown={(e) => {
          if (!canvas) return;
          const p = pointerPosition(e);
          const v = viewport();
          drag = { pointerId: e.pointerId, startX: p.x, startY: p.y, panX: v.panX, panY: v.panY, moved: false };
          canvas.setPointerCapture(e.pointerId);
          canvas.style.cursor = "grabbing";
          setDragging(true);
          setHovered(null);
        }}
        onPointerMove={(e) => {
          const p = pointerPosition(e);
          if (drag) {
            const dx = p.x - drag.startX;
            const dy = p.y - drag.startY;
            if (Math.hypot(dx, dy) > 3) drag.moved = true;
            setViewport({ ...viewport(), panX: drag.panX + dx, panY: drag.panY + dy });
            canvas!.style.cursor = "grabbing";
            return;
          }
          updateHover(e);
        }}
        onPointerUp={(e) => {
          if (!canvas || !drag) return;
          suppressClick = drag.moved;
          try { canvas.releasePointerCapture(drag.pointerId); } catch {}
          drag = null;
          setDragging(false);
          setHovered(null);
          canvas.style.cursor = "grab";
          if (suppressClick) e.preventDefault();
        }}
        onPointerCancel={() => {
          drag = null;
          setDragging(false);
          setHovered(null);
          if (canvas) canvas.style.cursor = "grab";
        }}
        onPointerLeave={() => {
          if (!dragging()) setHovered(null);
        }}
        onDblClick={() => setViewport(DEFAULT_BRAIN_VIEWPORT)}
        onClick={(e) => {
          if (suppressClick) {
            suppressClick = false;
            return;
          }
          const l = layout();
          if (!l || !canvas) return;
          const p = pointerPosition(e);
          const node = hitNode(l, canvas, p.x, p.y, viewport());
          if (!node) return;
          setHovered(null);
          props.onOpenDoc?.(node.id, node.title);
        }}
      />
      <Show when={!graph()}>
        <div class="lk-brain-map-state">
          <Show when={loadError()} fallback="loading corpus graph">
            graph offline · {loadError()}
          </Show>
        </div>
      </Show>
      <Show when={counts()}>
        {(c) => (
          <div class="lk-brain-map-stats">
            <span>{c().nodes} nodes</span>
            <span>{c().edges} links</span>
          </div>
        )}
      </Show>
    </div>
  );
}

export function BrainWidget(props: {
  scene: Scene;
  corpusTools: CorpusToolCall[];
  onOpenDoc?: (id: string, title: string) => void;
}) {
  const entries = createMemo(() =>
    uniqueEntries(props.corpusTools.flatMap(parseCorpusHits))
  );

  return (
    <div class="lk-brain-widget">
      <div class="lk-brain-head">
        <span class="label"><b>CORPUS</b> · 大脑</span>
        <span class="meta">{sceneMeta(props.scene)}</span>
      </div>
      <div class="lk-brain-body">
        <CorpusGraphMap
          scene={props.scene}
          entries={entries()}
          onOpenDoc={(id, title) => props.onOpenDoc?.(id, title)}
        />
      </div>
    </div>
  );
}

export function InsightSidebar(props: {
  scene: Scene;
  corpusTools: CorpusToolCall[];
  plan?: AgentPlanView | null;
  onOpenDoc?: (id: string, title: string) => void;
}) {
  return (
    <aside class="lk-insight-sidebar">
      <BrainWidget
        scene={props.scene}
        corpusTools={props.corpusTools}
        onOpenDoc={props.onOpenDoc}
      />
      <Show when={props.plan && props.plan.items.length > 0}>
        <AgentPlanWidget plan={props.plan!} />
      </Show>
    </aside>
  );
}

function planStatusLabel(status: string): string {
  switch (status) {
    case "pending": return "待办";
    case "in_progress": return "进行中";
    case "completed": return "已完成";
    case "blocked": return "受阻";
    case "skipped": return "已跳过";
    default: return status.replace("_", " ");
  }
}

function AgentPlanWidget(props: { plan: AgentPlanView }) {
  const completed = createMemo(() =>
    props.plan.items.filter((item) => item.status === "completed").length
  );
  const active = createMemo(() =>
    props.plan.items.find((item) => item.status === "in_progress")
  );
  const meta = createMemo(() => {
    const total = props.plan.items.length;
    if (completed() === total) return "已完成";
    if (active()) return "进行中";
    return "待开始";
  });

  return (
    <div class="lk-plan-widget">
      <div class="lk-plan-head">
        <span class="label"><b>团队</b> · 计划</span>
        <span class="meta">{completed()}/{props.plan.items.length} · {meta()}</span>
      </div>
      <div class="lk-plan-body">
        <Show when={props.plan.explanation}>
          <div class="lk-plan-explain">{props.plan.explanation}</div>
        </Show>
        <div class="lk-plan-list">
          <For each={props.plan.items}>{(item) => (
            <div class={`lk-plan-item is-${item.status}`}>
              <span class="lk-plan-mark" aria-hidden="true" />
              <div class="lk-plan-copy">
                <div class="lk-plan-step">{item.step}</div>
                <div class="lk-plan-meta">
                  <span>{planStatusLabel(item.status)}</span>
                  <Show when={item.evidence}>
                    <span title={item.evidence ?? ""}>{item.evidence}</span>
                  </Show>
                </div>
              </div>
            </div>
          )}</For>
        </div>
      </div>
    </div>
  );
}
