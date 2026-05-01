// L.E.E.K Workbench — main shell. 2 columns (chat | canvas), brain widget at top-right.

const { useEffect: __useEffect, useRef: __useRef, useMemo: __useMemo } = React;

function TopBar({ scene }) {
  const route = scene === "idle"
    ? "~/sessions/2026-04-29/scratch"
    : "~/sessions/2026-04-29/nvda-thesis";
  const session = scene === "idle" ? "untitled" : "NVDA · Pre-market thesis · v3";
  return (
    <div className="lk-top">
      <span className="lk-top-brand"><b>L.E.E.K</b> · LOGIC-ENHANCED EQUITY KERNEL</span>
      <span className="lk-top-sep">/</span>
      <span className="lk-top-route">{route}</span>
      <div className="lk-top-spacer" />
      <span className="lk-top-stat"><span className="lk-pulse" /> MKT OPEN · NYSE</span>
      <span className="lk-top-stat">SPX <b>5,612.41</b> <span style={{ color: "var(--up)" }}>+0.42%</span></span>
      <span className="lk-top-stat">NDX <b>19,884.10</b> <span style={{ color: "var(--up)" }}>+0.71%</span></span>
      <span className="lk-top-stat">VIX <b>14.62</b> <span style={{ color: "var(--down)" }}>−1.8%</span></span>
    </div>
  );
}

function Rail({ active = "chat" }) {
  const items = [
    { k: "chat",     icon: "chat",   label: "Chat" },
    { k: "canvas",   icon: "canvas", label: "Canvas" },
    { k: "corpus",   icon: "book",   label: "Corpus" },
    { k: "screener", icon: "grid",   label: "Screener" },
    { k: "agents",   icon: "branch", label: "Agents" },
  ];
  return (
    <aside className="lk-rail">
      <div className="lk-rail-logo">L</div>
      {items.map(it => (
        <button key={it.k} className="lk-rail-btn" data-active={active === it.k} title={it.label}>
          <Icon name={it.icon} />
        </button>
      ))}
      <div className="lk-rail-spacer" />
      <button className="lk-rail-btn" title="Settings"><Icon name="settings" /></button>
      <div className="lk-rail-avatar">JC</div>
    </aside>
  );
}

/* ------ Brain widget (top-right of canvas) ------ */
function BrainWidget({ scene, fireIds }) {
  const hostRef = __useRef(null);
  const apiRef  = __useRef(null);

  __useEffect(() => {
    if (!hostRef.current || !window.LeekBrain) return;
    apiRef.current = window.LeekBrain.mount(hostRef.current);
  }, []);

  // when scene changes / fireIds change, fire those nodes
  __useEffect(() => {
    if (!apiRef.current || !fireIds) return;
    apiRef.current.fire(fireIds);
    // schedule a sequence for thinking/deep
    if (scene === "thinking-shallow" || scene === "deep") {
      let i = 0;
      const id = setInterval(() => {
        if (!apiRef.current) return;
        apiRef.current.fire([fireIds[i % fireIds.length]]);
        i++;
      }, 1300);
      return () => clearInterval(id);
    }
  }, [scene, fireIds && fireIds.join(",")]);

  const meta = scene === "deep"     ? "indexing · live"
            : scene === "thinking-shallow" ? "indexing · live"
            : scene === "delivered" ? "reasoning · cached"
            : scene === "clarify"   ? "ambient"
            :                         "ambient";

  const stats = scene === "deep" ? [
    { k: "QUERY",   v: "NVDA + capex" },
    { k: "ACTIVE",  v: "12" },
    { k: "DEPTH",   v: "3" },
  ] : scene === "delivered" ? [
    { k: "REFS",    v: "9" },
    { k: "CONF",    v: "0.78" },
    { k: "DRIFT",   v: "−0.04" },
  ] : [
    { k: "DOCS",    v: "14,238" },
    { k: "VECS",    v: "2.1M" },
    { k: "TIERS",   v: "P·K" },
  ];

  return (
    <div className="lk-brain-widget" ref={hostRef}>
      <div className="lk-brain-head">
        <span className="label"><b>CORPUS</b> · BRAIN</span>
        <span className="meta">{meta}</span>
      </div>
      <div className="lk-brain-legend">
        <span className="it"><span className="sw" style={{ background: "var(--c-prin-wiki)" }} />principles · wikis</span>
        <span className="it"><span className="sw" style={{ background: "var(--c-prin-src)"  }} />principles · sources</span>
        <span className="it"><span className="sw" style={{ background: "var(--c-know-wiki)" }} />knowledge · wikis</span>
        <span className="it"><span className="sw" style={{ background: "var(--c-know-src)"  }} />knowledge · sources</span>
      </div>
      <div className="lk-brain-stats">
        {stats.map(s => (
          <span className="row" key={s.k}>
            <span className="k">{s.k}</span>
            <span className="v">{s.v}</span>
          </span>
        ))}
      </div>
    </div>
  );
}

/* ============ scene → panel layouts ============ */

const NVDA_QUOTE = {
  sym: "NVDA", price: 942.18, chg: 25.92, chgPct: 2.84,
  ts: "09:42:21 ET", venue: "NASDAQ"
};

function CanvasIdle() {
  return (
    <div className="lk-canvas-empty">
      <div className="label">CANVAS · IDLE</div>
      <div className="sub">Reasoning artifacts will materialize here.</div>
    </div>
  );
}

function CanvasThinkingShallow() {
  // Just two small panels appearing — quote + a sub-agent that's still working
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
        modules={[{ kind: "news", items: [
          { ts: "09:42 ET", src: "RTRS", head: "Citi reiterates NVDA Buy, raises PT to $1,050",     imp: "high" },
          { ts: "09:38 ET", src: "BBG",  head: "AI capex chatter: hyperscaler 2026 prelim guides",   imp: "med"  },
          { ts: "09:31 ET", src: "RTRS", head: "Pre-market: NVDA +2.8%, AMD +0.6%, MU −0.4%",        imp: "low"  },
        ] }]}
      />
      <Panel
        kind="subagent"
        title="research · cross-check"
        x={36} y={342} w={420} h={120}
        state="loading"
        animDelay={420}
        modules={[{ kind: "sub", data: {
          role: "research-agent",
          task: "Search principles/wikis for moat + late-cycle capex signals",
          progress: 0.42, step: 3, total: 7, tools: 4, elapsed: "1.4s"
        } }]}
      />
    </>
  );
}

function CanvasClarify() {
  // user has been asked to disambiguate; canvas keeps its panels visible
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
          { kind: "kv", title: "EXISTING POSITION", rows: [
            ["Symbol",     "NVDA"],
            ["Avg cost",   "$612.40"],
            ["Mkt value",  "$74,219.40", "hi"],
            ["Unrealized", "+53.8%", "up"],
            ["NAV %",      "8.4%", "hi"],
            ["Risk band",  "B-2 (high vol, sized)"],
          ] }
        ]}
      />
      <Panel
        kind="corpus"
        title="rules in scope"
        x={356} y={70} w={360} h={276}
        animDelay={120}
        modules={[{ kind: "cites", items: [
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/margin-of-safety.md",
            title: "Margin of Safety",
            quote: "When you build a bridge, you insist it can carry 30,000 pounds, but you only drive 10,000-pound trucks across it." },
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/concentration-over-diversification.md",
            title: "Concentration over Diversification",
            quote: "Wide diversification is only required when investors do not understand what they are doing." },
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/risk-as-permanent-loss-not-volatility.md",
            title: "Risk as permanent loss, not volatility" },
          { tier: "prin-src",
            path: "principles/sources/buffett/letters/2007.md",
            title: "Berkshire 2007 letter — moats, durability" },
        ] }]}
      />
    </>
  );
}

function CanvasDeep() {
  // The big show: 6 panels assembling, brain firing
  const ageStartedAgo = (s) => 200 + s * 90; // staggered fade-in
  return (
    <>
      {/* Row 1 — quote · subagent · news */}
      <Panel
        kind="quote-card"
        title="NVDA · spot"
        x={28} y={70} w={232} h={86}
        modules={[{ kind: "quote", data: NVDA_QUOTE }]}
        animDelay={ageStartedAgo(0)}
      />
      <Panel
        kind="subagent"
        title="comparables"
        x={272} y={70} w={244} h={86}
        state="loading"
        animDelay={ageStartedAgo(1)}
        modules={[{ kind: "sub", data: {
          role: "comparables-agent",
          task: "Peer-set: AMD, MU on AI capex elasticity",
          progress: 0.74, step: 5, total: 7, tools: 6, elapsed: "2.1s"
        } }]}
      />
      <Panel
        kind="subagent"
        title="synthesis"
        x={528} y={70} w={244} h={86}
        state="loading"
        animDelay={ageStartedAgo(2)}
        modules={[{ kind: "sub", data: {
          role: "synthesis-agent",
          task: "Stitch principles + knowledge into thesis",
          progress: 0.31, step: 2, total: 6, tools: 3, elapsed: "1.8s"
        } }]}
      />

      {/* Row 2 — chart full-width-ish (avoids brain zone in y<280 by staying x<770) */}
      <Panel
        kind="chart"
        title="NVDA · 3M OHLC"
        sub="3M · daily"
        x={28} y={170} w={744} h={236}
        animDelay={ageStartedAgo(3)}
        modules={[{ kind: "candles", w: 744, h: 196, seed: 7, base: 880, sym: "NVDA", price: 942.18 }]}
      />

      {/* Row 3 — primary doc | corpus refs | comparables */}
      <Panel
        kind="evidence"
        title="primary doc · 10-Q pp.18"
        sub="highlighted"
        x={28} y={420} w={494} h={228}
        animDelay={ageStartedAgo(4)}
        modules={[{ kind: "pdf",
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
            { before: "Data Center revenue grew ",
              mark: "154% YoY",
              after: " driven by Hopper-architecture demand and continued ramp of HGX platform deliveries.",
              cite: "NVIDIA · 10-Q Q2 FY25 · Item 2 · pp.18 · ¶4.2" },
            { before: "Compute & Networking operating income increased ",
              mark: "+174% YoY",
              after: " on operating leverage from gross-margin mix shift toward H200/HGX configurations.",
              cite: "NVIDIA · 10-Q Q2 FY25 · Item 2 · pp.19 · ¶6.1" },
          ]
        }]}
      />

      <Panel
        kind="corpus"
        title="corpus refs (8)"
        sub="principles + knowledge"
        x={534} y={420} w={234} h={228}
        animDelay={ageStartedAgo(5)}
        modules={[{ kind: "cites", items: [
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/economic-moat.md",
            title: "Economic Moat" },
          { tier: "know-wiki",
            path: "knowledge/wikis/concepts/hyperscaler-ai-capex-cycle.md",
            title: "Hyperscaler AI capex cycle" },
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/long-term-debt-cycle.md",
            title: "Long-term debt cycle" },
          { tier: "know-wiki",
            path: "knowledge/wikis/entities/nvidia.md",
            title: "NVIDIA — entity wiki" },
          { tier: "know-src",
            path: "knowledge/sources/ai-stack/nvda-q2-fy25-10q.md",
            title: "NVDA Q2 FY25 10-Q" },
          { tier: "prin-src",
            path: "principles/sources/buffett/letters/2007.md",
            title: "Berkshire 2007 letter" },
        ] }]}
      />

      <Panel
        kind="compare"
        title="comparables · AI semis"
        sub="TTM"
        x={780} y={420} w={234} h={228}
        animDelay={ageStartedAgo(6)}
        modules={[{ kind: "cmp",
          headers: ["", "NVDA", "AMD", "MU"],
          rows: [
            { hl: true, cells: [
              { v: "Rev YoY" }, { v: "+122%", cls: "up hi" },
              { v: "+9%",  cls: "up" }, { v: "+82%", cls: "up" },
            ] },
            { cells: [
              { v: "Gross M" }, { v: "75.1%", cls: "hi" },
              { v: "47.6%" }, { v: "29.4%" },
            ] },
            { cells: [
              { v: "Op M" }, { v: "62.0%", cls: "hi" },
              { v: "12.7%" }, { v: "13.2%" },
            ] },
            { cells: [
              { v: "FCF/rev" }, { v: "44.9%", cls: "hi" },
              { v: "11.0%" }, { v: "8.4%" },
            ] },
            { cells: [
              { v: "P/E (FWD)" }, { v: "38.2×" },
              { v: "29.4×" }, { v: "11.7×" },
            ] },
            { cells: [
              { v: "Capex sens" }, { v: "high", cls: "dn" },
              { v: "high", cls: "dn" }, { v: "high", cls: "dn" },
            ] },
          ]
        }]}
      />
    </>
  );
}

function CanvasDelivered() {
  return (
    <>
      {/* row 1 */}
      <Panel
        kind="quote-card"
        title="NVDA · spot"
        x={28} y={70} w={232} h={86}
        modules={[{ kind: "quote", data: NVDA_QUOTE }]}
      />
      <Panel
        kind="chart"
        title="NVDA · 3M + IV"
        x={272} y={70} w={420} h={236}
        animDelay={60}
        modules={[{ kind: "candles", w: 420, h: 196, seed: 7, base: 880, sym: "NVDA", price: 942.18 }]}
      />
      <Panel
        kind="valuation"
        title="valuation ladder"
        sub="1.2% NAV cap · +4% Q3"
        x={704} y={70} w={232} h={236}
        animDelay={120}
        modules={[{ kind: "valuation",
          steps: [
            { k: "Spot",                 v: "$942.18" },
            { k: "Justified pop (Q2 fund.)", v: "+2.2%" },
            { k: "Citi premium",         v: "+0.6%" },
            { k: "Risk-band haircut",    v: "−1.4%" },
            { k: "12-mo fair value",     v: "$1,008" },
            { tot: true, k: "Δ to spot",    v: "+7.0%" },
          ]
        }]}
      />

      {/* row 2 */}
      <Panel
        kind="evidence"
        title="primary doc · 10-Q pp.18"
        x={28} y={320} w={420} h={210}
        animDelay={180}
        modules={[{ kind: "pdf",
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
            { before: "Data Center revenue grew ", mark: "154% YoY",
              after: " driven by Hopper architecture demand and continued ramp of HGX platform deliveries.",
              cite: "NVIDIA · 10-Q Q2 FY25 · Item 2 · pp.18" },
          ]
        }]}
      />

      <Panel
        kind="corpus"
        title="principles lens · 4 refs"
        x={460} y={320} w={460} h={210}
        animDelay={240}
        modules={[{ kind: "cites", items: [
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/economic-moat.md",
            title: "Economic Moat — moat intact: CUDA + Hopper",
            quote: "A truly great business must have an enduring moat that protects excellent returns on capital." },
          { tier: "prin-wiki",
            path: "principles/wikis/concepts/long-term-debt-cycle.md",
            title: "Long-term debt cycle — late-cycle capex risk" },
          { tier: "know-wiki",
            path: "knowledge/wikis/concepts/hyperscaler-ai-capex-cycle.md",
            title: "Hyperscaler capex cycle — 2026 elasticity" },
          { tier: "prin-src",
            path: "principles/sources/buffett/letters/2007.md",
            title: "Berkshire 2007 letter — durability test" },
        ] }]}
      />

      {/* row 3 — decision */}
      <Panel
        kind="decision"
        title="DecisionDraft · BUY NVDA"
        x={28} y={544} w={892} h={120}
        animDelay={320}
        modules={[{ kind: "decision", data: {
          verdict: "BUY",
          sym: "NVDA",
          confidence: 0.74,
          gist: "Re-sized to 1.2% NAV at limit ≤ $946. Justified pop ~2.2%, Citi premium ~60bps. Late-cycle capex risk hedged by sizing not avoidance.",
          params: [
            { k: "ENTRY",    v: "limit ≤ $946.00" },
            { k: "SIZE",     v: "1.2% NAV  (~$4,950)" },
            { k: "STOP",     v: "−7.5% close, $874.51" },
            { k: "EXIT",     v: "+15% or thesis break" },
            { k: "HORIZON",  v: "Position · 3–12m" },
            { k: "RULESET",  v: "principles/wikis/margin-of-safety.md" },
          ]
        } }]}
      />
    </>
  );
}

function CanvasArea({ scene }) {
  return (
    <div className="lk-canvas">
      {scene !== "idle" && (
        <div className="lk-canvas-head">
          <span className="crumb">
            <b>{scene === "delivered" ? "NVDA · Pre-market thesis · v3" : "NVDA · Pre-market thesis"}</b>
            <span className="sep">/</span>
            {scene === "delivered" ? "verdict · cached"
              : scene === "deep"      ? "reasoning · live · 6 panels"
              : scene === "thinking-shallow" ? "reasoning · live"
              : scene === "clarify"   ? "awaiting input"
              : "no thread"}
          </span>
          <div className="actions">
            <button className="iconbtn" title="Layers"><Icon name="layer" className="ic-sm" /></button>
            <button className="iconbtn" title="Search"><Icon name="search" className="ic-sm" /></button>
            <button className="iconbtn" title="Expand"><Icon name="expand" className="ic-sm" /></button>
          </div>
        </div>
      )}

      {scene === "idle" && <CanvasIdle />}
      {scene === "thinking-shallow" && <CanvasThinkingShallow />}
      {scene === "clarify" && <CanvasClarify />}
      {scene === "deep" && <CanvasDeep />}
      {scene === "delivered" && <CanvasDelivered />}

      <BrainWidget
        scene={scene}
        fireIds={
          scene === "deep" ? [
            "p.economic-moat", "k.nvidia", "p.long-term-debt-cycle",
            "k.hyperscaler-capex", "s.dalio.bigdebt", "p.margin-of-safety",
            "p.buffett", "ks.ai.10qs"
          ]
          : scene === "thinking-shallow" ? [
            "k.nvidia", "p.paradigm-shifts", "k.hyperscaler-capex"
          ]
          : scene === "delivered" ? [
            "p.economic-moat", "p.long-term-debt-cycle", "k.hyperscaler-capex"
          ]
          : scene === "clarify" ? [
            "p.margin-of-safety", "p.concentration"
          ]
          : null
        }
      />
    </div>
  );
}

/* --------------- Workbench --------------- */

function Workbench({ scene = "idle" }) {
  let transcript;
  if (scene === "idle")             transcript = <TranscriptIdle />;
  else if (scene === "thinking-shallow") transcript = <TranscriptThinkingShallow />;
  else if (scene === "clarify")     transcript = <TranscriptClarify />;
  else if (scene === "deep")        transcript = <TranscriptDeepResearch />;
  else if (scene === "delivered")   transcript = <TranscriptDelivered />;

  const composerVal =
    scene === "thinking-shallow" ? ""
    : scene === "clarify"        ? "Position (3–12m), 1–3% NAV"
    : scene === "deep"           ? ""
    : scene === "delivered"      ? "What if Q3 is +4% above guide instead?"
    : "";

  const headTitle =
    scene === "idle" ? "NEW SESSION"
    : scene === "delivered" ? "CHAT · NVDA THESIS · v3"
    : "CHAT · NVDA THESIS";
  const headMeta =
    scene === "idle" ? "0 turns"
    : scene === "thinking-shallow" ? "2 turns · just now"
    : scene === "clarify" ? "3 turns · 09:46"
    : scene === "deep" ? "2 turns · live"
    : "5 turns · 09:42";

  return (
    <div className="lk-app" data-screen-label={`L.E.E.K · ${scene}`}>
      <Rail active={scene === "idle" ? "chat" : "chat"} />
      <div className="lk-main">
        <TopBar scene={scene} />
        <section className="lk-chat">
          <div className="lk-chat-head">
            <span className="lk-chat-head-title">{headTitle}</span>
            <span className="lk-chat-head-meta">{headMeta}</span>
          </div>
          <div className="lk-chat-body">
            {transcript}
          </div>
          <Composer value={composerVal} focus={scene === "delivered"} />
        </section>
        <CanvasArea scene={scene} />
      </div>
    </div>
  );
}

window.Workbench = Workbench;
