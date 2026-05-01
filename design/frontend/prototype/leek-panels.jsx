// Composable panel system. A panel is a chrome-wrapped stack of modules.
// Modules are small typed renderers; the agent assembles them at runtime.
//
//   <Panel kind="quote-card" title="NVDA · Quote" x={...} y={...} w={...} h={...}
//          modules={[{kind:'quote', data:{...}}, {kind:'kv', data:{...}}, ...]} />

const { useMemo: _useMemo } = React;

/* ============= module renderers ============= */

function ModQuote({ data }) {
  const ch = data.chgPct;
  return (
    <div className="lk-mod-quote">
      <span className="sym">{data.sym}</span>
      <span className="px">{data.price.toFixed(2)}</span>
      <span className={"ch " + (ch >= 0 ? "up" : "dn")}>
        {ch >= 0 ? "+" : ""}{data.chg.toFixed(2)} ({ch >= 0 ? "+" : ""}{ch.toFixed(2)}%)
      </span>
      <span className="meta">{data.venue} · {data.ts}</span>
    </div>
  );
}

function ModKV({ title, rows }) {
  return (
    <div className="lk-mod lk-mod-kv">
      {title && <div className="lk-mod-head">{title}</div>}
      <table>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              <td>{r[0]}</td>
              <td className={r[2] || ""}>{r[1]}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/* deterministic candle generator — same shape regardless of caller */
function _genCandles(seed, n, base) {
  let s = seed;
  const rand = () => { s = (s * 9301 + 49297) % 233280; return s / 233280; };
  const out = [];
  let close = base;
  for (let i = 0; i < n; i++) {
    const drift = (rand() - 0.40) * 11;
    const open = close;
    close = Math.max(20, open + drift);
    const hi = Math.max(open, close) + rand() * 5.2;
    const lo = Math.min(open, close) - rand() * 5.2;
    const vol = 0.35 + rand() * 0.6;
    out.push({ open, close, hi, lo, vol });
  }
  return out;
}

function ModCandles({ w, h, seed = 7, base = 880, sym, price, range = "3M" }) {
  const candles = _useMemo(() => _genCandles(seed, 64, base), [seed, base]);
  const W = w, H = h;
  const padL = 10, padR = 48, padT = 8, padB = 26;
  const innerW = W - padL - padR;
  const innerH = H - padT - padB;
  const volH = 22;
  const priceH = innerH - volH - 4;

  let lo = Infinity, hi = -Infinity;
  for (const c of candles) { lo = Math.min(lo, c.lo); hi = Math.max(hi, c.hi); }
  const pad = (hi - lo) * 0.10; lo -= pad; hi += pad;
  const xStep = innerW / candles.length;
  const yScale = (p) => padT + (1 - (p - lo) / (hi - lo)) * priceH;
  const bodyW = Math.max(1.6, xStep * 0.60);

  const ticks = 4;
  const tickVals = Array.from({ length: ticks + 1 }, (_, i) => lo + (hi - lo) * (i / ticks));
  const xLabels = ["MAR", "APR", "MAY", "JUN", "JUL"];

  const last = candles[candles.length - 1].close;
  const px = price != null ? price : last;

  return (
    <div className="lk-mod lk-mod-chart">
      <svg width={W} height={H} style={{ display: "block" }}>
        <defs>
          <linearGradient id={`cup-${seed}`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stopColor="#6fb98a" stopOpacity="1" />
            <stop offset="100%" stopColor="#6fb98a" stopOpacity="0.85" />
          </linearGradient>
          <linearGradient id={`cdn-${seed}`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stopColor="#d97070" stopOpacity="1" />
            <stop offset="100%" stopColor="#d97070" stopOpacity="0.85" />
          </linearGradient>
        </defs>

        {tickVals.map((v, i) => (
          <g key={i}>
            <line x1={padL} x2={W - padR} y1={yScale(v)} y2={yScale(v)} stroke="rgba(255,240,220,0.05)" />
            <text x={W - padR + 5} y={yScale(v) + 3}
              fill="rgba(168,158,140,0.65)" fontSize="9" fontFamily="JetBrains Mono, monospace">
              {v.toFixed(0)}
            </text>
          </g>
        ))}

        {candles.map((c, i) => {
          const cx = padL + xStep * (i + 0.5);
          const up = c.close >= c.open;
          const yO = yScale(c.open), yC = yScale(c.close);
          const yH = yScale(c.hi), yL = yScale(c.lo);
          const fill = up ? `url(#cup-${seed})` : `url(#cdn-${seed})`;
          const stroke = up ? "#6fb98a" : "#d97070";
          return (
            <g key={i}>
              <line x1={cx} x2={cx} y1={yH} y2={yL} stroke={stroke} strokeWidth="0.7" />
              <rect
                x={cx - bodyW / 2}
                y={Math.min(yO, yC)}
                width={bodyW}
                height={Math.max(1, Math.abs(yC - yO))}
                fill={fill}
                stroke={stroke}
                strokeWidth="0.4"
              />
            </g>
          );
        })}

        {/* close line */}
        <path
          d={candles.map((c, i) => `${i === 0 ? "M" : "L"} ${padL + xStep * (i + 0.5)} ${yScale(c.close)}`).join(" ")}
          fill="none" stroke="rgba(217,119,87,0.6)" strokeWidth="0.9" strokeDasharray="2 2"
        />

        {candles.map((c, i) => {
          const cx = padL + xStep * (i + 0.5);
          const up = c.close >= c.open;
          const vh = c.vol * volH;
          const yTop = padT + priceH + 4 + (volH - vh);
          return (
            <rect key={"v" + i}
              x={cx - bodyW / 2} y={yTop}
              width={bodyW} height={vh}
              fill={up ? "rgba(111,185,138,0.40)" : "rgba(217,112,112,0.40)"}
            />
          );
        })}

        {xLabels.map((m, i) => (
          <text key={m}
            x={padL + (innerW * (i / (xLabels.length - 1)))}
            y={H - 8}
            fill="rgba(168,158,140,0.6)" fontSize="9" fontFamily="JetBrains Mono, monospace"
            textAnchor={i === 0 ? "start" : (i === xLabels.length - 1 ? "end" : "middle")}>
            {m}
          </text>
        ))}

        {/* current price marker */}
        <g>
          <line x1={padL} x2={W - padR} y1={yScale(px)} y2={yScale(px)}
            stroke="rgba(217,119,87,0.55)" strokeWidth="0.8" strokeDasharray="3 3" />
          <rect x={W - padR + 1} y={yScale(px) - 7} width="44" height="14" rx="2"
            fill="rgba(217,119,87,0.18)" stroke="rgba(217,119,87,0.55)" />
          <text x={W - padR + 23} y={yScale(px) + 3.5}
            fill="#e89a7e" fontSize="9.5" fontFamily="JetBrains Mono, monospace" textAnchor="middle">
            {px.toFixed(2)}
          </text>
        </g>
      </svg>
    </div>
  );
}

function ModPdfPage({ doc, snippets, citation, highlights }) {
  return (
    <div className="lk-mod lk-mod-pdf">
      <div className="page">
        <h6>{doc.title}</h6>
        <p>{doc.intro}</p>
        <p>{doc.body1}</p>
        {doc.highlight && <p><span className="hl">{doc.highlight}</span></p>}
        <p>{doc.body2}</p>
        <p>{doc.body3}</p>
        <span className="lbl">{doc.lbl}</span>
      </div>
      <div className="extract">
        {snippets.map((s, i) => (
          <div key={i}>
            <div className="qt">"{s.before}<mark>{s.mark}</mark>{s.after}"</div>
            <div className="cite">— {s.cite}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

function ModCites({ items }) {
  // each item: { tier: 'prin-wiki' | 'prin-src' | 'know-wiki' | 'know-src',
  //              path: 'principles/wikis/concepts/margin-of-safety.md',
  //              title: 'Margin of Safety',
  //              quote: '…' }
  return (
    <div className="lk-mod-cites">
      {items.map((it, i) => (
        <div key={i} className="cite-row" data-tier={it.tier}>
          <span className="pin" />
          <div className="body">
            <div className="path">{it.path}</div>
            <div className="title">{it.title}</div>
            {it.quote && <div className="qt">"{it.quote}"</div>}
          </div>
        </div>
      ))}
    </div>
  );
}

function ModNews({ items }) {
  return (
    <div className="lk-mod-news">
      {items.map((it, i) => (
        <div key={i} className="item">
          <span className="ts">{it.ts}</span>
          <span className="head">
            <span className="src">[{it.src}]</span>
            {it.head}
          </span>
          <span className={"imp " + (it.imp === "high" ? "hi" : "")}>{it.imp.toUpperCase()}</span>
        </div>
      ))}
    </div>
  );
}

function ModRows({ rows }) {
  return (
    <div className="lk-mod lk-mod-rows">
      {rows.map((r, i) => {
        // tiny sparkline
        const seed = (r.sym || '').charCodeAt(0) || 7;
        const candles = _genCandles(seed, 24, 80);
        let lo = Infinity, hi = -Infinity;
        candles.forEach(c => { lo = Math.min(lo, c.lo); hi = Math.max(hi, c.hi); });
        const w = 60, h = 14;
        const path = candles.map((c, j) => {
          const x = (j / (candles.length - 1)) * w;
          const y = h - ((c.close - lo) / (hi - lo)) * h;
          return `${j === 0 ? 'M' : 'L'}${x.toFixed(1)},${y.toFixed(1)}`;
        }).join(' ');
        return (
          <div key={i} className="row">
            <span className="sym">{r.sym}</span>
            <span className="name">{r.name}</span>
            <svg className="spark" width={w} height={h}>
              <path d={path} fill="none" stroke={r.ch >= 0 ? "#6fb98a" : "#d97070"} strokeWidth="1" />
            </svg>
            <span className="px">{r.px.toFixed(2)}</span>
            <span className={"ch " + (r.ch >= 0 ? "up" : "dn")}>{r.ch >= 0 ? "+" : ""}{r.ch.toFixed(2)}%</span>
          </div>
        );
      })}
    </div>
  );
}

function ModCmpTable({ headers, rows }) {
  return (
    <div className="lk-mod lk-mod-cmp">
      <table>
        <thead><tr>{headers.map((h, i) => <th key={i}>{h}</th>)}</tr></thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i} className={r.hl ? "hl" : ""}>
              {r.cells.map((c, j) => (
                <td key={j} className={c.cls || ""}>{c.v}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ModSubAgent({ data }) {
  return (
    <div className="lk-mod lk-mod-sub">
      <div className="role">◆ {data.role}</div>
      <div className="task">{data.task}</div>
      <div className="meter"><i style={{ width: `${Math.round(data.progress * 100)}%` }} /></div>
      <div className="meta">step {data.step}/{data.total} · {data.tools} tool calls · {data.elapsed}</div>
    </div>
  );
}

function ModValuation({ steps, target, current }) {
  return (
    <div className="lk-mod lk-mod-val">
      <div className="ladder">
        {steps.map((s, i) => (
          <div key={i} className={"step " + (s.tot ? "tot" : "")}>
            <span className="lbl">{s.k}</span>
            <span className="v">{s.v}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function ModDecision({ data, onPin }) {
  return (
    <div className="lk-mod lk-mod-dec">
      <div className="head">
        <span className={"verdict " + data.verdict.toLowerCase()}>{data.verdict}</span>
        <span className="sym">{data.sym}</span>
        <span className="conf">CONF · <b>{data.confidence.toFixed(2)}</b></span>
      </div>
      <div className="gist">{data.gist}</div>
      <div className="params">
        {data.params.map((p, i) => (
          <div key={i} className="p">
            <span className="k">{p.k}</span>
            <span className="v">{p.v}</span>
          </div>
        ))}
      </div>
      <div className="actions">
        <button className="btn primary">REVIEW &amp; SUBMIT</button>
        <button className="btn ghost">EDIT PARAMS</button>
        <button className="btn ghost">PIN ↗</button>
      </div>
    </div>
  );
}

/* ============ Panel chrome ============ */

const KIND_META = {
  "quote-card":     { kind: "QUOTE",       dot: "var(--n-quote)" },
  "fundamentals":   { kind: "FUNDAMENTALS",dot: "var(--n-fund)" },
  "chart":          { kind: "CHART_OHLC",  dot: "var(--n-chart)" },
  "evidence":       { kind: "PRIMARY_DOC", dot: "var(--clay-soft)" },
  "corpus":         { kind: "CORPUS_REFS", dot: "var(--n-corpus)" },
  "compare":        { kind: "COMPARE",     dot: "var(--n-fund)" },
  "news":           { kind: "NEWS_FEED",   dot: "var(--amber)" },
  "subagent":       { kind: "SUBAGENT",    dot: "var(--n-sub)" },
  "valuation":      { kind: "VALUATION",   dot: "var(--n-fund)" },
  "decision":       { kind: "DECISION",    dot: "var(--n-decision)" },
  "watch":          { kind: "WATCH",       dot: "var(--ink-1)" },
};

function Panel({ kind, title, sub, x, y, w, h, modules, state, animDelay = 0, actions = true }) {
  const meta = KIND_META[kind] || { kind: kind.toUpperCase(), dot: "var(--clay)" };
  const style = {
    left: x, top: y, width: w, height: h,
    animationDelay: animDelay + "ms",
  };
  return (
    <div className="lk-panel" data-state={state || "idle"} style={style}>
      <div className="lk-panel-head">
        <span className="kind">
          <span className="dot" style={{ background: meta.dot, boxShadow: `0 0 6px ${meta.dot}` }} />
          {meta.kind}
        </span>
        <span className="title">{title}</span>
        {sub && <span className="sub">{sub}</span>}
        {actions && (
          <span className="actions">
            <button title="Pin"><Icon name="pin" className="ic-xs" /></button>
            <button title="Expand"><Icon name="expand" className="ic-xs" /></button>
            <button title="Close"><Icon name="close" className="ic-xs" /></button>
          </span>
        )}
      </div>
      <div className="lk-panel-body">
        {modules.map((m, i) => {
          if (m.kind === "quote")    return <ModQuote     key={i} data={m.data} />;
          if (m.kind === "kv")       return <ModKV        key={i} title={m.title} rows={m.rows} />;
          if (m.kind === "candles")  return <ModCandles   key={i} {...m} />;
          if (m.kind === "pdf")      return <ModPdfPage   key={i} {...m} />;
          if (m.kind === "cites")    return <ModCites     key={i} items={m.items} />;
          if (m.kind === "news")     return <ModNews      key={i} items={m.items} />;
          if (m.kind === "rows")     return <ModRows      key={i} rows={m.rows} />;
          if (m.kind === "cmp")      return <ModCmpTable  key={i} headers={m.headers} rows={m.rows} />;
          if (m.kind === "sub")      return <ModSubAgent  key={i} data={m.data} />;
          if (m.kind === "valuation")return <ModValuation key={i} {...m} />;
          if (m.kind === "decision") return <ModDecision  key={i} data={m.data} />;
          return null;
        })}
      </div>
    </div>
  );
}

Object.assign(window, { Panel });
