// Chat column — messages, agent thinking trace, clarifications, token-stream reveals.

const { useState: _useState, useEffect: _useEffect, useRef: _useRef } = React;

/* token-stream rendering: split into spans, each animates in
   with staggered delay; if `live`, leave a caret blinking at end. */
function StreamText({ text, startMs = 0, perTok = 22, live = false, splitOn = /(\s+)/ }) {
  const parts = text.split(splitOn).filter(s => s !== "");
  return (
    <>
      {parts.map((p, i) => (
        <span
          key={i}
          className="lk-tok"
          style={{ animationDelay: (startMs + i * perTok) + "ms" }}
        >{p}</span>
      ))}
      {live && <span className="lk-stream" />}
    </>
  );
}

function NodeRefPill({ type, id, label, onClick }) {
  return (
    <span className="lk-noderef" data-t={type} onClick={onClick}>
      <span className="dot" />
      <span style={{ color: "var(--ink-3)" }}>{id}</span>
      <span>{label}</span>
    </span>
  );
}

function CorpusCite({ tier = "principles", path, children }) {
  return (
    <span className="lk-cite" data-tier={tier}>
      <span style={{ color: "var(--ink-3)" }}>↪</span>
      {path}
    </span>
  );
}

function MsgHead({ who, role, time }) {
  return (
    <div className="lk-msg-head" data-who={who}>
      <span className="who">{role}</span>
      <span>· {time}</span>
    </div>
  );
}

function UserMsg({ time, children }) {
  return (
    <div className="lk-msg">
      <MsgHead who="user" role="YOU" time={time} />
      <div className="lk-msg-body user">{children}</div>
    </div>
  );
}

function AgentMsg({ time, children }) {
  return (
    <div className="lk-msg">
      <MsgHead who="agent" role="L.E.E.K" time={time} />
      <div className="lk-msg-body">{children}</div>
    </div>
  );
}

function SystemMsg({ time, children }) {
  return (
    <div className="lk-msg">
      <MsgHead who="system" role="SYSTEM" time={time} />
      <div className="lk-msg-body" style={{ fontSize: 11.5, color: "var(--ink-2)", fontFamily: "var(--font-mono)" }}>{children}</div>
    </div>
  );
}

function TraceBlock({ steps }) {
  return (
    <div className="lk-trace">
      {steps.map((s, i) => (
        <div key={i} className={"lk-trace-step " + s.state}>
          <span className="tag">{s.tag}</span>
          {s.src && <span className="src">{s.src}</span>}
          <span>{s.text}</span>
          {s.ms != null && <span className="ms">{s.ms}ms</span>}
        </div>
      ))}
    </div>
  );
}

function ClarifyCard({ question, opts, picked }) {
  return (
    <div className="lk-clarify">
      <div className="lk-clarify-q">
        <b>L.E.E.K asks ·</b> {question}
      </div>
      <div className="lk-clarify-opts">
        {opts.map(o => (
          <button key={o} className={"lk-chip " + (picked === o ? "active" : "")}>{o}</button>
        ))}
      </div>
    </div>
  );
}

function Composer({ value, placeholder = "Ask the kernel — query a name, draft a thesis, or run a screen…", focus = false }) {
  return (
    <div className="lk-composer">
      <div className="lk-composer-box" style={focus ? { borderColor: "rgba(217,119,87,0.45)", boxShadow: "0 0 0 3px rgba(217,119,87,0.07)" } : null}>
        <textarea defaultValue={value} placeholder={placeholder} rows={2} />
        <div className="lk-composer-row">
          <button className="lk-composer-tool" title="Attach"><Icon name="paperclip" className="ic-sm" /></button>
          <button className="lk-composer-tool" title="Reference"><Icon name="pin" className="ic-sm" /></button>
          <button className="lk-composer-tool" title="Voice"><Icon name="mic" className="ic-sm" /></button>
          <span className="lk-chip-mini">corpus · 14.2K docs</span>
          <span className="lk-chip-mini">tools · 9</span>
          <button className="lk-composer-send"><Icon name="send" className="ic-xs" /> SEND</button>
        </div>
      </div>
      <div className="lk-composer-hint" style={{ marginTop: 8 }}>
        <span className="kbd">↵</span> send
        <span className="kbd">⇧↵</span> newline
        <span style={{ marginLeft: "auto" }}>Logic-Enhanced Equity Kernel · v0.4.1</span>
      </div>
    </div>
  );
}

/* ============= scene transcripts ============= */

function TranscriptIdle() {
  return (
    <>
      <SystemMsg time="09:31:02">
        Session opened. Corpus indexed: 14,238 docs · principles + knowledge tiers · paper portfolio loaded.
      </SystemMsg>
      <AgentMsg time="09:31:04">
        Markets opened 14 min ago. Watchlist mixed — semis green, energy flat, no flagged corpus events.
        <br />Tell me what to look at.
      </AgentMsg>
    </>
  );
}

function TranscriptThinkingShallow() {
  return (
    <>
      <UserMsg time="09:42:18">
        Why is NVDA up 2.8% pre-market?
      </UserMsg>
      <AgentMsg time="09:42:19">
        <StreamText text="One sec — pulling the tape and the corpus." perTok={18} />
        <div style={{ marginTop: 8 }}>
          <TraceBlock steps={[
            { tag: "QUOTE",  text: "fetch NVDA · NASDAQ", ms: 88, state: "done" },
            { tag: "NEWS",   text: "scan 09:30 → now", ms: 142, state: "done" },
            { tag: "CORPUS", text: "principles/wikis · paradigm-shifts, hyperscaler-capex", state: "run" },
          ]} />
        </div>
      </AgentMsg>
    </>
  );
}

function TranscriptClarify() {
  return (
    <>
      <UserMsg time="09:46:40">Should I buy more NVDA?</UserMsg>
      <AgentMsg time="09:46:41">
        Before I draft a position, two things.
        <div style={{ marginTop: 8, display: "flex", flexDirection: "column", gap: 8 }}>
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
        <div style={{ marginTop: 10, fontSize: 12, color: "var(--ink-2)" }}>
          Defaults pulled from your <CorpusCite tier="principles" path="principles/wikis/concepts/margin-of-safety.md" />
          and <CorpusCite tier="principles" path="principles/wikis/concepts/concentration-over-diversification.md" />.
        </div>
      </AgentMsg>
    </>
  );
}

function TranscriptDeepResearch() {
  return (
    <>
      <UserMsg time="09:42:18">
        NVDA up 2.8% pre-market on a Citi note. Is the move justified by fundamentals, or sell-side noise?
        I want both Buffett-on-moats and Dalio-on-capex-cycles in the answer.
      </UserMsg>
      <AgentMsg time="09:42:19">
        <StreamText text="Working. Decomposing into four threads — quote + Q2 10-Q, the Citi note, the principles lens (moat + late-cycle capex), and a comparable on AMD/MU." perTok={11} />
        <div style={{ marginTop: 10 }}>
          <TraceBlock steps={[
            { tag: "PLAN",    text: "split: tape · primary docs · principles · comparables", ms: 41, state: "done" },
            { tag: "TOOL",    text: "quote(NVDA) → $942.18 +2.84%", ms: 88, state: "done" },
            { tag: "TOOL",    text: "10-Q Q2 FY25 · pp.18 · Hopper revenue", ms: 412, state: "done" },
            { tag: "CORPUS",  src: "principles/wikis", text: "economic-moat · circle-of-competence · margin-of-safety", ms: 167, state: "done" },
            { tag: "CORPUS",  src: "knowledge/wikis",  text: "nvidia · gpu-economics · hyperscaler-capex · dc-power", ms: 184, state: "done" },
            { tag: "CORPUS",  src: "principles/sources", text: "buffett · 2007/2011 letters · moats", ms: 96, state: "done" },
            { tag: "SUB",     text: "comparables-agent: AMD / MU peer set", state: "run" },
            { tag: "SUB",     text: "synthesis-agent: stitching draft", state: "run" },
          ]} />
        </div>
        <div style={{ marginTop: 8, fontSize: 12, color: "var(--ink-2)" }}>
          Surfacing artifacts on canvas as I go. <span className="lk-stream" />
        </div>
      </AgentMsg>
    </>
  );
}

function TranscriptDelivered() {
  return (
    <>
      <UserMsg time="09:42:18">
        NVDA up 2.8% pre-market on a Citi note. Justified by fundamentals or sell-side noise?
        Want Buffett-on-moats and Dalio-on-capex.
      </UserMsg>
      <AgentMsg time="09:42:51">
        Verdict: <em className="up">justified, partially.</em> The move is reasonable on the 10-Q;
        Citi adds about <em className="tk">60bps</em> of extra noise on top of fair value.
        <ol style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 12.5, color: "var(--ink-1)", lineHeight: 1.6 }}>
          <li>
            Q2 revenue <em className="up">+122% YoY</em>, op-margin <em className="up">62%</em> — see{" "}
            <NodeRefPill type="pdf" id="P-02" label="10-Q · pp.18" />.
          </li>
          <li>
            Justified pop is <em className="q">~2.2%</em>, not 2.8% — see{" "}
            <NodeRefPill type="chart" id="P-03" label="OHLC + IV" />.
          </li>
          <li>
            Citi rests on a Q3 revenue assumption <em className="dn">+8% above</em> guidance — see{" "}
            <NodeRefPill type="news" id="P-04" label="Citi note · 08-12" />.
          </li>
        </ol>
        <div style={{ marginTop: 10, fontSize: 12.5, lineHeight: 1.55 }}>
          <span style={{ color: "var(--clay-soft)", fontFamily: "var(--font-mono)", fontSize: 10.5, letterSpacing: ".05em" }}>PRINCIPLES LENS ·</span>
          {" "}The moat is intact (CUDA + Hopper)
          <CorpusCite tier="principles" path="principles/wikis/concepts/economic-moat.md" />
          but you&apos;re late in a hyperscaler capex cycle that
          <CorpusCite tier="principles" path="principles/wikis/concepts/long-term-debt-cycle.md" />
          and{" "}
          <CorpusCite tier="knowledge" path="knowledge/wikis/concepts/hyperscaler-ai-capex-cycle.md" />
          frame as a known late-stage paradigm-shift trap.
        </div>
        <div style={{ marginTop: 10, fontSize: 12.5, lineHeight: 1.55 }}>
          Drafted a position — <NodeRefPill type="decision" id="P-06" label="DecisionDraft · BUY 1.8% NAV" />. Nothing fires until you confirm.
        </div>
      </AgentMsg>
      <UserMsg time="09:43:42">
        What if Q3 actually comes in 4% above guide instead of 8%? And cap me at 1.2%.
      </UserMsg>
      <AgentMsg time="09:43:43">
        <StreamText text="Re-running valuation at +4% Q3 surprise, sizing capped at 1.2% NAV." live perTok={20} />
      </AgentMsg>
    </>
  );
}

Object.assign(window, {
  Composer,
  TranscriptIdle, TranscriptThinkingShallow, TranscriptClarify,
  TranscriptDeepResearch, TranscriptDelivered,
});
