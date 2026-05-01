// 5 scene transcripts shown in the chat column.
// Ported from prototype/leek-chat.jsx → SolidJS.

import { AgentMsg, ClarifyCard, CorpusCite, NodeRefPill, StreamText, SystemMsg, TraceBlock, UserMsg } from "./Chat";
import type { Scene } from "../scenes";

export function TranscriptIdle() {
  return (
    <>
      <SystemMsg time="09:31:02">
        Session opened. Corpus indexed: 14,238 docs · principles + knowledge tiers · paper portfolio loaded.
      </SystemMsg>
      <AgentMsg time="09:31:04">
        <>
          Markets opened 14 min ago. Watchlist mixed — semis green, energy flat, no flagged corpus events.
          <br />Tell me what to look at.
        </>
      </AgentMsg>
    </>
  );
}

export function TranscriptThinkingShallow() {
  return (
    <>
      <UserMsg time="09:42:18">Why is NVDA up 2.8% pre-market?</UserMsg>
      <AgentMsg time="09:42:19">
        <>
          <StreamText text="One sec — pulling the tape and the corpus." perTok={18} />
          <div style={{ "margin-top": "8px" }}>
            <TraceBlock steps={[
              { tag: "QUOTE", text: "fetch NVDA · NASDAQ", ms: 88, state: "done" },
              { tag: "NEWS", text: "scan 09:30 → now", ms: 142, state: "done" },
              { tag: "CORPUS", text: "principles/wikis · paradigm-shifts, hyperscaler-capex", state: "run" },
            ]} />
          </div>
        </>
      </AgentMsg>
    </>
  );
}

export function TranscriptClarify() {
  return (
    <>
      <UserMsg time="09:46:40">Should I buy more NVDA?</UserMsg>
      <AgentMsg time="09:46:41">
        <>
          Before I draft a position, two things.
          <div style={{ "margin-top": "8px", display: "flex", "flex-direction": "column", gap: "8px" }}>
            <ClarifyCard
              question="Time horizon?"
              opts={["Intraday", "Swing (1–4w)", "Position (3–12m)", "Long-term (1y+)"]}
              picked="Position (3–12m)"
            />
            <ClarifyCard
              question="Risk envelope?"
              opts={["≤1% NAV", "1–3% NAV", "3–5% NAV", "Use existing rule"]}
              picked="1–3% NAV"
            />
          </div>
          <div style={{ "margin-top": "10px", "font-size": "12px", color: "var(--ink-2)" }}>
            Defaults pulled from your{" "}
            <CorpusCite tier="principles" path="principles/wikis/concepts/margin-of-safety.md" />
            {" "}and{" "}
            <CorpusCite tier="principles" path="principles/wikis/concepts/concentration-over-diversification.md" />.
          </div>
        </>
      </AgentMsg>
    </>
  );
}

export function TranscriptDeepResearch() {
  return (
    <>
      <UserMsg time="09:42:18">
        <>
          NVDA up 2.8% pre-market on a Citi note. Is the move justified by fundamentals, or sell-side noise?
          I want both Buffett-on-moats and Dalio-on-capex-cycles in the answer.
        </>
      </UserMsg>
      <AgentMsg time="09:42:19">
        <>
          <StreamText text="Working. Decomposing into four threads — quote + Q2 10-Q, the Citi note, the principles lens (moat + late-cycle capex), and a comparable on AMD/MU." perTok={11} />
          <div style={{ "margin-top": "10px" }}>
            <TraceBlock steps={[
              { tag: "PLAN", text: "split: tape · primary docs · principles · comparables", ms: 41, state: "done" },
              { tag: "TOOL", text: "quote(NVDA) → $942.18 +2.84%", ms: 88, state: "done" },
              { tag: "TOOL", text: "10-Q Q2 FY25 · pp.18 · Hopper revenue", ms: 412, state: "done" },
              { tag: "CORPUS", src: "principles/wikis", text: "economic-moat · circle-of-competence · margin-of-safety", ms: 167, state: "done" },
              { tag: "CORPUS", src: "knowledge/wikis", text: "nvidia · gpu-economics · hyperscaler-capex · dc-power", ms: 184, state: "done" },
              { tag: "CORPUS", src: "principles/sources", text: "buffett · 2007/2011 letters · moats", ms: 96, state: "done" },
              { tag: "SUB", text: "comparables-agent: AMD / MU peer set", state: "run" },
              { tag: "SUB", text: "synthesis-agent: stitching draft", state: "run" },
            ]} />
          </div>
          <div style={{ "margin-top": "8px", "font-size": "12px", color: "var(--ink-2)" }}>
            Surfacing artifacts on canvas as I go. <span class="lk-stream" />
          </div>
        </>
      </AgentMsg>
    </>
  );
}

export function TranscriptDelivered() {
  return (
    <>
      <UserMsg time="09:42:18">
        <>
          NVDA up 2.8% pre-market on a Citi note. Justified by fundamentals or sell-side noise?
          Want Buffett-on-moats and Dalio-on-capex.
        </>
      </UserMsg>
      <AgentMsg time="09:42:51">
        <>
          Verdict: <em class="up">justified, partially.</em> The move is reasonable on the 10-Q;
          Citi adds about <em class="tk">60bps</em> of extra noise on top of fair value.
          <ol style={{ margin: "8px 0 0", "padding-left": "18px", "font-size": "12.5px", color: "var(--ink-1)", "line-height": 1.6 }}>
            <li>
              Q2 revenue <em class="up">+122% YoY</em>, op-margin <em class="up">62%</em> — see{" "}
              <NodeRefPill type="pdf" id="P-02" label="10-Q · pp.18" />.
            </li>
            <li>
              Justified pop is <em class="q">~2.2%</em>, not 2.8% — see{" "}
              <NodeRefPill type="chart" id="P-03" label="OHLC + IV" />.
            </li>
            <li>
              Citi rests on a Q3 revenue assumption <em class="dn">+8% above</em> guidance — see{" "}
              <NodeRefPill type="news" id="P-04" label="Citi note · 08-12" />.
            </li>
          </ol>
          <div style={{ "margin-top": "10px", "font-size": "12.5px", "line-height": 1.55 }}>
            <span style={{ color: "var(--clay-soft)", "font-family": "var(--font-mono)", "font-size": "10.5px", "letter-spacing": ".05em" }}>PRINCIPLES LENS ·</span>
            {" "}The moat is intact (CUDA + Hopper)
            <CorpusCite tier="principles" path="principles/wikis/concepts/economic-moat.md" />
            but you're late in a hyperscaler capex cycle that
            <CorpusCite tier="principles" path="principles/wikis/concepts/long-term-debt-cycle.md" />
            and{" "}
            <CorpusCite tier="knowledge" path="knowledge/wikis/concepts/hyperscaler-ai-capex-cycle.md" />
            frame as a known late-stage paradigm-shift trap.
          </div>
          <div style={{ "margin-top": "10px", "font-size": "12.5px", "line-height": 1.55 }}>
            Drafted a position — <NodeRefPill type="decision" id="P-06" label="DecisionDraft · BUY 1.8% NAV" />. Nothing fires until you confirm.
          </div>
        </>
      </AgentMsg>
      <UserMsg time="09:43:42">What if Q3 actually comes in 4% above guide instead of 8%? And cap me at 1.2%.</UserMsg>
      <AgentMsg time="09:43:43">
        <StreamText text="Re-running valuation at +4% Q3 surprise, sizing capped at 1.2% NAV." live perTok={20} />
      </AgentMsg>
    </>
  );
}

export function transcriptFor(scene: Scene) {
  switch (scene) {
    case "idle": return <TranscriptIdle />;
    case "thinking-shallow": return <TranscriptThinkingShallow />;
    case "clarify": return <TranscriptClarify />;
    case "deep": return <TranscriptDeepResearch />;
    case "delivered": return <TranscriptDelivered />;
  }
}
