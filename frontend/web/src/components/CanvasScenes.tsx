// 5 canvas scene layouts (panel placement on the workbench canvas).
// Ported from prototype/leek-workbench.jsx → SolidJS.

import { Panel } from "./Panel";
import type { Scene } from "../scenes";

const NVDA_QUOTE = {
  sym: "NVDA", price: 942.18, chg: 25.92, chgPct: 2.84,
  ts: "09:42:21 ET", venue: "NASDAQ",
};

export function CanvasIdle() {
  return (
    <div class="lk-canvas-empty">
      <div class="label">CANVAS · IDLE</div>
      <div class="sub">推理产物会在这里逐步浮现。</div>
    </div>
  );
}

export function CanvasThinkingShallow() {
  return (
    <>
      <Panel
        kind="quote-card"
        title="NVDA · spot"
        x={36} y={70} w={300} h={86}
        modules={[{ kind: "quote", data: NVDA_QUOTE }]}
        animDelay={50}
      />
      <Panel
        kind="news"
        title="Tape · 09:30 → now"
        x={36} y={176} w={420} h={146}
        animDelay={220}
        modules={[{
          kind: "news", items: [
            { ts: "09:42 ET", src: "RTRS", head: "Citi reiterates NVDA Buy, raises PT to $1,050", imp: "high" },
            { ts: "09:38 ET", src: "BBG", head: "AI capex chatter: hyperscaler 2026 prelim guides", imp: "med" },
            { ts: "09:31 ET", src: "RTRS", head: "Pre-market: NVDA +2.8%, AMD +0.6%, MU −0.4%", imp: "low" },
          ]
        }]}
      />
      <Panel
        kind="subagent"
        title="research · cross-check"
        x={36} y={342} w={420} h={120}
        state="loading"
        animDelay={420}
        modules={[{
          kind: "sub", data: {
            role: "research-agent",
            task: "Search principles/wikis for moat + late-cycle capex signals",
            progress: 0.42, step: 3, total: 7, tools: 4, elapsed: "1.4s",
          }
        }]}
      />
    </>
  );
}

export function CanvasClarify() {
  return (
    <>
      <Panel
        kind="quote-card"
        title="NVDA · spot"
        x={36} y={70} w={300} h={86}
        modules={[{ kind: "quote", data: NVDA_QUOTE }]}
      />
      <Panel
        kind="watch"
        title="position context"
        x={36} y={176} w={300} h={170}
        modules={[
          {
            kind: "kv", title: "EXISTING POSITION", rows: [
              ["Symbol", "NVDA"],
              ["Avg cost", "$612.40"],
              ["Mkt value", "$74,219.40", "hi"],
              ["Unrealized", "+53.8%", "up"],
              ["NAV %", "8.4%", "hi"],
              ["Risk band", "B-2 (high vol, sized)"],
            ]
          }
        ]}
      />
      <Panel
        kind="corpus"
        title="rules in scope"
        x={356} y={70} w={360} h={276}
        animDelay={120}
        modules={[{
          kind: "cites", items: [
            {
              tier: "prin-wiki",
              path: "principles/wikis/concepts/margin-of-safety.md",
              title: "Margin of Safety",
              quote: "When you build a bridge, you insist it can carry 30,000 pounds, but you only drive 10,000-pound trucks across it."
            },
            {
              tier: "prin-wiki",
              path: "principles/wikis/concepts/concentration-over-diversification.md",
              title: "Concentration over Diversification",
              quote: "Wide diversification is only required when investors do not understand what they are doing."
            },
            {
              tier: "prin-wiki",
              path: "principles/wikis/concepts/risk-as-permanent-loss-not-volatility.md",
              title: "Risk as permanent loss, not volatility"
            },
            {
              tier: "prin-src",
              path: "principles/sources/buffett/letters/2007.md",
              title: "Berkshire 2007 letter — moats, durability"
            },
          ]
        }]}
      />
    </>
  );
}

export function CanvasDeep() {
  const ageStartedAgo = (s: number) => 200 + s * 90;
  return (
    <>
      <Panel
        kind="quote-card" title="NVDA · spot"
        x={28} y={70} w={232} h={86}
        modules={[{ kind: "quote", data: NVDA_QUOTE }]}
        animDelay={ageStartedAgo(0)}
      />
      <Panel
        kind="subagent" title="comparables"
        x={272} y={70} w={244} h={86}
        state="loading"
        animDelay={ageStartedAgo(1)}
        modules={[{
          kind: "sub", data: {
            role: "comparables-agent",
            task: "Peer-set: AMD, MU on AI capex elasticity",
            progress: 0.74, step: 5, total: 7, tools: 6, elapsed: "2.1s",
          }
        }]}
      />
      <Panel
        kind="subagent" title="synthesis"
        x={528} y={70} w={244} h={86}
        state="loading"
        animDelay={ageStartedAgo(2)}
        modules={[{
          kind: "sub", data: {
            role: "synthesis-agent",
            task: "Stitch principles + knowledge into thesis",
            progress: 0.31, step: 2, total: 6, tools: 3, elapsed: "1.8s",
          }
        }]}
      />

      <Panel
        kind="chart" title="NVDA · 3M OHLC" sub="3M · daily"
        x={28} y={170} w={744} h={236}
        animDelay={ageStartedAgo(3)}
        modules={[{ kind: "candles", w: 744, h: 196, seed: 7, base: 880, sym: "NVDA", price: 942.18 }]}
      />

      <Panel
        kind="evidence" title="primary doc · 10-Q pp.18" sub="highlighted"
        x={28} y={420} w={494} h={228}
        animDelay={ageStartedAgo(4)}
        modules={[{
          kind: "pdf",
          doc: {
            title: "NVIDIA · Form 10-Q · Q2 FY2025",
            intro: "Item 2 — Management's Discussion and Analysis of Financial Condition and Results of Operations.",
            body1: "During the second quarter of fiscal 2025, our Data Center revenue was $26.3 billion, a sequential increase of 16% and a year-over-year increase of 154%, primarily driven by demand for our Hopper architecture and continued ramp of our HGX systems.",
            highlight: "Data Center revenue grew 154% year-over-year driven by Hopper-architecture demand and continued ramp of HGX platform deliveries.",
            body2: "Networking revenue was $3.7 billion, up 114% year-over-year. Gaming revenue was $2.9 billion, roughly flat year-over-year. Compute & Networking segment operating income was $19.6 billion, an increase of 174%.",
            body3: "We anticipate continued strong demand for our products through fiscal 2025, supported by hyperscaler customer commitments…",
            lbl: "p.18",
          },
          snippets: [
            {
              before: "Data Center revenue grew ", mark: "154% YoY",
              after: " driven by Hopper-architecture demand and continued ramp of HGX platform deliveries.",
              cite: "NVIDIA · 10-Q Q2 FY25 · Item 2 · pp.18 · ¶4.2"
            },
            {
              before: "Compute & Networking operating income increased ", mark: "+174% YoY",
              after: " on operating leverage from gross-margin mix shift toward H200/HGX configurations.",
              cite: "NVIDIA · 10-Q Q2 FY25 · Item 2 · pp.19 · ¶6.1"
            },
          ]
        }]}
      />

      <Panel
        kind="corpus" title="corpus refs (8)" sub="principles + knowledge"
        x={534} y={420} w={234} h={228}
        animDelay={ageStartedAgo(5)}
        modules={[{
          kind: "cites", items: [
            { tier: "prin-wiki", path: "principles/wikis/concepts/economic-moat.md", title: "Economic Moat" },
            { tier: "know-wiki", path: "knowledge/wikis/concepts/hyperscaler-ai-capex-cycle.md", title: "Hyperscaler AI capex cycle" },
            { tier: "prin-wiki", path: "principles/wikis/concepts/long-term-debt-cycle.md", title: "Long-term debt cycle" },
            { tier: "know-wiki", path: "knowledge/wikis/entities/nvidia.md", title: "NVIDIA — entity wiki" },
            { tier: "know-src", path: "knowledge/sources/ai-stack/nvda-q2-fy25-10q.md", title: "NVDA Q2 FY25 10-Q" },
            { tier: "prin-src", path: "principles/sources/buffett/letters/2007.md", title: "Berkshire 2007 letter" },
          ]
        }]}
      />

      <Panel
        kind="compare" title="comparables · AI semis" sub="TTM"
        x={780} y={420} w={234} h={228}
        animDelay={ageStartedAgo(6)}
        modules={[{
          kind: "cmp",
          headers: ["", "NVDA", "AMD", "MU"],
          rows: [
            { hl: true, cells: [{ v: "Rev YoY" }, { v: "+122%", cls: "up hi" }, { v: "+9%", cls: "up" }, { v: "+82%", cls: "up" }] },
            { cells: [{ v: "Gross M" }, { v: "75.1%", cls: "hi" }, { v: "47.6%" }, { v: "29.4%" }] },
            { cells: [{ v: "Op M" }, { v: "62.0%", cls: "hi" }, { v: "12.7%" }, { v: "13.2%" }] },
            { cells: [{ v: "FCF/rev" }, { v: "44.9%", cls: "hi" }, { v: "11.0%" }, { v: "8.4%" }] },
            { cells: [{ v: "P/E (FWD)" }, { v: "38.2×" }, { v: "29.4×" }, { v: "11.7×" }] },
            { cells: [{ v: "Capex sens" }, { v: "high", cls: "dn" }, { v: "high", cls: "dn" }, { v: "high", cls: "dn" }] },
          ]
        }]}
      />
    </>
  );
}

export function CanvasDelivered() {
  return (
    <>
      <Panel
        kind="quote-card" title="NVDA · spot"
        x={28} y={70} w={232} h={86}
        modules={[{ kind: "quote", data: NVDA_QUOTE }]}
      />
      <Panel
        kind="chart" title="NVDA · 3M + IV"
        x={272} y={70} w={420} h={236}
        animDelay={60}
        modules={[{ kind: "candles", w: 420, h: 196, seed: 7, base: 880, sym: "NVDA", price: 942.18 }]}
      />
      <Panel
        kind="valuation" title="valuation ladder" sub="1.2% NAV cap · +4% Q3"
        x={704} y={70} w={232} h={236}
        animDelay={120}
        modules={[{
          kind: "valuation",
          steps: [
            { k: "Spot", v: "$942.18" },
            { k: "Justified pop (Q2 fund.)", v: "+2.2%" },
            { k: "Citi premium", v: "+0.6%" },
            { k: "Risk-band haircut", v: "−1.4%" },
            { k: "12-mo fair value", v: "$1,008" },
            { tot: true, k: "Δ to spot", v: "+7.0%" },
          ]
        }]}
      />

      <Panel
        kind="evidence" title="primary doc · 10-Q pp.18"
        x={28} y={320} w={420} h={210}
        animDelay={180}
        modules={[{
          kind: "pdf",
          doc: {
            title: "NVIDIA · 10-Q · Q2 FY25",
            intro: "Item 2. MD&A.",
            body1: "Data Center revenue $26.3B, +16% q/q and +154% y/y, driven by Hopper architecture demand and continued ramp of HGX systems.",
            highlight: "Data Center revenue grew 154% YoY driven by Hopper-architecture demand and HGX ramp.",
            body2: "Networking revenue $3.7B, +114% YoY. Compute & Networking op income $19.6B, +174%.",
            body3: "Strong continued demand expected through fiscal 2025 supported by hyperscaler commitments.",
            lbl: "p.18",
          },
          snippets: [
            {
              before: "Data Center revenue grew ", mark: "154% YoY",
              after: " driven by Hopper architecture demand and continued ramp of HGX platform deliveries.",
              cite: "NVIDIA · 10-Q Q2 FY25 · Item 2 · pp.18"
            },
          ]
        }]}
      />

      <Panel
        kind="corpus" title="principles lens · 4 refs"
        x={460} y={320} w={460} h={210}
        animDelay={240}
        modules={[{
          kind: "cites", items: [
            {
              tier: "prin-wiki",
              path: "principles/wikis/concepts/economic-moat.md",
              title: "Economic Moat — moat intact: CUDA + Hopper",
              quote: "A truly great business must have an enduring moat that protects excellent returns on capital."
            },
            {
              tier: "prin-wiki",
              path: "principles/wikis/concepts/long-term-debt-cycle.md",
              title: "Long-term debt cycle — late-cycle capex risk"
            },
            {
              tier: "know-wiki",
              path: "knowledge/wikis/concepts/hyperscaler-ai-capex-cycle.md",
              title: "Hyperscaler capex cycle — 2026 elasticity"
            },
            {
              tier: "prin-src",
              path: "principles/sources/buffett/letters/2007.md",
              title: "Berkshire 2007 letter — durability test"
            },
          ]
        }]}
      />

      <Panel
        kind="decision" title="DecisionDraft · BUY NVDA"
        x={28} y={544} w={892} h={120}
        animDelay={320}
        modules={[{
          kind: "decision", data: {
            verdict: "BUY",
            sym: "NVDA",
            confidence: 0.74,
            gist: "Re-sized to 1.2% NAV at limit ≤ $946. Justified pop ~2.2%, Citi premium ~60bps. Late-cycle capex risk hedged by sizing not avoidance.",
            params: [
              { k: "ENTRY", v: "limit ≤ $946.00" },
              { k: "SIZE", v: "1.2% NAV  (~$4,950)" },
              { k: "STOP", v: "−7.5% close, $874.51" },
              { k: "EXIT", v: "+15% or thesis break" },
              { k: "HORIZON", v: "Position · 3–12m" },
              { k: "RULESET", v: "principles/wikis/margin-of-safety.md" },
            ]
          }
        }]}
      />
    </>
  );
}

export function canvasFor(scene: Scene) {
  switch (scene) {
    case "idle": return <CanvasIdle />;
    case "thinking-shallow": return <CanvasThinkingShallow />;
    case "clarify": return <CanvasClarify />;
    case "deep": return <CanvasDeep />;
    case "delivered": return <CanvasDelivered />;
  }
}
