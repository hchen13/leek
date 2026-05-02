/* CorpusBrain v2 — 2D force-directed graph rendered to <canvas>
   Nodes are real LEEK corpus files (principles/wikis, principles/sources,
   knowledge/wikis, knowledge/sources). Cross-tier edges represent
   "this wiki cites this source", "this concept appears in this entity", etc.
   Designed to live in a 340x340 widget at top-right of canvas. */

(function () {
  'use strict';

  // ----- node data: real paths from corpus/ -----
  // tier codes: pw = principles/wikis, ps = principles/sources,
  //             kw = knowledge/wikis, ks = knowledge/sources
  const NODES = [
    // principles · wikis · concepts (durable mental models)
    { id: 'p.margin-of-safety',         t: 'pw', l: 'margin-of-safety',         g: 'concepts' },
    { id: 'p.economic-moat',            t: 'pw', l: 'economic-moat',            g: 'concepts' },
    { id: 'p.circle-of-competence',     t: 'pw', l: 'circle-of-competence',     g: 'concepts' },
    { id: 'p.mr-market',                t: 'pw', l: 'mr-market',                g: 'concepts' },
    { id: 'p.owner-earnings',           t: 'pw', l: 'owner-earnings',           g: 'concepts' },
    { id: 'p.compound-interest',        t: 'pw', l: 'compound-interest',        g: 'concepts' },
    { id: 'p.opportunity-cost',         t: 'pw', l: 'opportunity-cost',         g: 'concepts' },
    { id: 'p.long-term-debt-cycle',     t: 'pw', l: 'long-term-debt-cycle',     g: 'concepts' },
    { id: 'p.short-term-debt-cycle',    t: 'pw', l: 'short-term-debt-cycle',    g: 'concepts' },
    { id: 'p.beautiful-deleveraging',   t: 'pw', l: 'beautiful-deleveraging',   g: 'concepts' },
    { id: 'p.economic-machine',         t: 'pw', l: 'economic-machine',         g: 'concepts' },
    { id: 'p.paradigm-shifts',          t: 'pw', l: 'paradigm-shifts',          g: 'concepts' },
    { id: 'p.risk-as-permanent-loss',   t: 'pw', l: 'risk-as-permanent-loss',   g: 'concepts' },
    { id: 'p.lollapalooza-effect',      t: 'pw', l: 'lollapalooza-effect',      g: 'concepts' },
    { id: 'p.psychology-misjudgment',   t: 'pw', l: 'psychology-of-misjudgment',g: 'concepts' },
    { id: 'p.stocks-as-businesses',     t: 'pw', l: 'stocks-as-businesses',     g: 'concepts' },
    { id: 'p.concentration',            t: 'pw', l: 'concentration-over-diversification', g: 'concepts' },
    { id: 'p.checklist-method',         t: 'pw', l: 'checklist-method',         g: 'concepts' },
    { id: 'p.inversion',                t: 'pw', l: 'inversion',                g: 'concepts' },
    { id: 'p.lessons-of-history',       t: 'pw', l: 'lessons-of-history',       g: 'concepts' },
    { id: 'p.populism-cycle',           t: 'pw', l: 'populism-cycle',           g: 'concepts' },
    { id: 'p.reserve-currency-cycle',   t: 'pw', l: 'reserve-currency-cycle',   g: 'concepts' },

    // principles · wikis · entities
    { id: 'p.buffett',                  t: 'pw', l: 'warren-buffett',           g: 'entities' },
    { id: 'p.munger',                   t: 'pw', l: 'charlie-munger',           g: 'entities' },
    { id: 'p.dalio',                    t: 'pw', l: 'ray-dalio',                g: 'entities' },

    // principles · sources
    { id: 's.dalio.machine',            t: 'ps', l: 'how-the-economic-machine-works' },
    { id: 's.dalio.bigdebt',            t: 'ps', l: 'big-debt-crises' },
    { id: 's.dalio.cwo',                t: 'ps', l: 'changing-world-order' },
    { id: 's.dalio.howcountries',       t: 'ps', l: 'how-countries-go-broke' },
    { id: 's.dalio.paradigms',          t: 'ps', l: 'paradigm-shifts' },
    { id: 's.dalio.populism',           t: 'ps', l: 'populism-the-phenomenon' },
    { id: 's.dalio.allweather',         t: 'ps', l: 'all-weather-new-world-2025' },
    { id: 's.dalio.principles',         t: 'ps', l: 'principles-life-and-work' },
    { id: 's.munger.psych',             t: 'ps', l: 'psychology-of-human-misjudgment-1995' },
    { id: 's.munger.almanack',          t: 'ps', l: "poor-charlie's-almanack" },
    { id: 's.munger.dj22',              t: 'ps', l: 'daily-journal-2022' },
    { id: 's.buffett.letters',          t: 'ps', l: 'berkshire-letters' },
    { id: 's.buffett.partnership',      t: 'ps', l: 'partnership-letters' },
    { id: 's.buffett.fortune',          t: 'ps', l: 'fortune-essays' },

    // knowledge · wikis (concepts)
    { id: 'k.ai-compute-stack',         t: 'kw', l: 'ai-compute-stack',         g: 'concepts' },
    { id: 'k.gpu-economics',            t: 'kw', l: 'gpu-accelerator-economics',g: 'concepts' },
    { id: 'k.hbm-bottleneck',           t: 'kw', l: 'hbm-memory-bandwidth',     g: 'concepts' },
    { id: 'k.train-vs-infer',           t: 'kw', l: 'training-vs-inference',    g: 'concepts' },
    { id: 'k.asic-vs-gpu',              t: 'kw', l: 'custom-asic-vs-merchant-gpu', g: 'concepts' },
    { id: 'k.hyperscaler-capex',        t: 'kw', l: 'hyperscaler-ai-capex-cycle', g: 'concepts' },
    { id: 'k.dc-power',                 t: 'kw', l: 'datacenter-power-constraints', g: 'concepts' },
    { id: 'k.ai-monetization',          t: 'kw', l: 'ai-monetization-vs-capex', g: 'concepts' },

    // knowledge · wikis (entities)
    { id: 'k.nvidia',                   t: 'kw', l: 'nvidia',                   g: 'entities' },
    { id: 'k.amd',                      t: 'kw', l: 'amd',                      g: 'entities' },
    { id: 'k.intel',                    t: 'kw', l: 'intel',                    g: 'entities' },
    { id: 'k.micron',                   t: 'kw', l: 'micron',                   g: 'entities' },
    { id: 'k.alphabet',                 t: 'kw', l: 'alphabet',                 g: 'entities' },

    // knowledge · sources
    { id: 'ks.opeds.coddle',            t: 'ks', l: '2011-stop-coddling',       g: 'op-eds' },
    { id: 'ks.opeds.minimumtax',        t: 'ks', l: '2012-minimum-tax',         g: 'op-eds' },
    { id: 'ks.opeds.bullishwomen',      t: 'ks', l: '2013-bullish-women',       g: 'op-eds' },
    { id: 'ks.opeds.pledge',            t: 'ks', l: '2010-philanthropic-pledge',g: 'op-eds' },
    { id: 'ks.policy.ubi',              t: 'ks', l: 'primer-on-ubi',            g: 'policy' },
    { id: 'ks.ai.10qs',                 t: 'ks', l: 'nvda-amd-mu-10q-filings',  g: 'industry' },
    { id: 'ks.ai.transcripts',          t: 'ks', l: 'earnings-transcripts',     g: 'industry' },
  ];

  // edges: real conceptual links — wiki cites source, entity uses concept, etc.
  const EDGES = [
    // buffett wiki <- buffett sources
    ['p.buffett',  's.buffett.letters'],
    ['p.buffett',  's.buffett.partnership'],
    ['p.buffett',  's.buffett.fortune'],
    ['p.munger',   's.munger.psych'],
    ['p.munger',   's.munger.almanack'],
    ['p.munger',   's.munger.dj22'],
    ['p.dalio',    's.dalio.principles'],
    ['p.dalio',    's.dalio.machine'],
    ['p.dalio',    's.dalio.cwo'],

    // concept wikis cite their primary sources
    ['p.economic-machine',         's.dalio.machine'],
    ['p.long-term-debt-cycle',     's.dalio.bigdebt'],
    ['p.long-term-debt-cycle',     's.dalio.howcountries'],
    ['p.short-term-debt-cycle',    's.dalio.machine'],
    ['p.beautiful-deleveraging',   's.dalio.bigdebt'],
    ['p.paradigm-shifts',          's.dalio.paradigms'],
    ['p.populism-cycle',           's.dalio.populism'],
    ['p.reserve-currency-cycle',   's.dalio.cwo'],
    ['p.lessons-of-history',       's.dalio.cwo'],
    ['p.psychology-misjudgment',   's.munger.psych'],
    ['p.lollapalooza-effect',      's.munger.psych'],
    ['p.lollapalooza-effect',      's.munger.almanack'],
    ['p.checklist-method',         's.munger.almanack'],
    ['p.inversion',                's.munger.almanack'],
    ['p.margin-of-safety',         's.buffett.letters'],
    ['p.economic-moat',            's.buffett.letters'],
    ['p.economic-moat',            's.buffett.fortune'],
    ['p.owner-earnings',           's.buffett.letters'],
    ['p.mr-market',                's.buffett.partnership'],
    ['p.mr-market',                's.buffett.letters'],
    ['p.circle-of-competence',     's.buffett.letters'],
    ['p.stocks-as-businesses',     's.buffett.partnership'],
    ['p.concentration',            's.buffett.partnership'],

    // concept-to-concept (mental model cross-links)
    ['p.margin-of-safety',         'p.risk-as-permanent-loss'],
    ['p.margin-of-safety',         'p.circle-of-competence'],
    ['p.economic-moat',            'p.stocks-as-businesses'],
    ['p.owner-earnings',           'p.stocks-as-businesses'],
    ['p.mr-market',                'p.opportunity-cost'],
    ['p.long-term-debt-cycle',     'p.short-term-debt-cycle'],
    ['p.long-term-debt-cycle',     'p.beautiful-deleveraging'],
    ['p.long-term-debt-cycle',     'p.economic-machine'],
    ['p.populism-cycle',           'p.long-term-debt-cycle'],
    ['p.reserve-currency-cycle',   'p.long-term-debt-cycle'],
    ['p.paradigm-shifts',          'p.lessons-of-history'],
    ['p.lollapalooza-effect',      'p.psychology-misjudgment'],
    ['p.checklist-method',         'p.psychology-misjudgment'],
    ['p.inversion',                'p.checklist-method'],

    // knowledge wikis - concepts to entities
    ['k.nvidia',                   'k.gpu-economics'],
    ['k.nvidia',                   'k.ai-compute-stack'],
    ['k.nvidia',                   'k.train-vs-infer'],
    ['k.amd',                      'k.gpu-economics'],
    ['k.amd',                      'k.ai-compute-stack'],
    ['k.amd',                      'k.asic-vs-gpu'],
    ['k.intel',                    'k.asic-vs-gpu'],
    ['k.micron',                   'k.hbm-bottleneck'],
    ['k.alphabet',                 'k.asic-vs-gpu'],
    ['k.alphabet',                 'k.hyperscaler-capex'],
    ['k.gpu-economics',            'k.hbm-bottleneck'],
    ['k.gpu-economics',            'k.train-vs-infer'],
    ['k.train-vs-infer',           'k.ai-monetization'],
    ['k.hyperscaler-capex',        'k.dc-power'],
    ['k.hyperscaler-capex',        'k.ai-monetization'],

    // knowledge wikis cite knowledge sources
    ['k.nvidia',                   'ks.ai.10qs'],
    ['k.nvidia',                   'ks.ai.transcripts'],
    ['k.amd',                      'ks.ai.10qs'],
    ['k.micron',                   'ks.ai.10qs'],
    ['k.hyperscaler-capex',        'ks.ai.transcripts'],
    ['k.dc-power',                 'ks.ai.transcripts'],

    // op-eds <-> buffett
    ['p.buffett',                  'ks.opeds.coddle'],
    ['p.buffett',                  'ks.opeds.minimumtax'],
    ['p.buffett',                  'ks.opeds.bullishwomen'],
    ['p.buffett',                  'ks.opeds.pledge'],

    // ********** the killer edges: principles ↔ knowledge **********
    ['p.economic-moat',            'k.nvidia'],          // CUDA moat
    ['p.circle-of-competence',     'k.ai-compute-stack'],
    ['p.long-term-debt-cycle',     'k.hyperscaler-capex'], // capex bubbles
    ['p.paradigm-shifts',          'k.train-vs-infer'],   // capex→opex shift
    ['p.opportunity-cost',         'k.gpu-economics'],
    ['p.margin-of-safety',         'k.hyperscaler-capex'],
  ];

  // ----- color tokens (read from CSS vars at runtime) -----
  function colors() {
    const css = getComputedStyle(document.documentElement);
    return {
      pw: css.getPropertyValue('--c-prin-wiki').trim() || '#d97757',
      ps: css.getPropertyValue('--c-prin-src').trim()  || '#a85636',
      kw: css.getPropertyValue('--c-know-wiki').trim() || '#e8b86c',
      ks: css.getPropertyValue('--c-know-src').trim()  || '#b88842',
      line: 'rgba(255, 240, 220, 0.07)',
      lineHi: 'rgba(217, 119, 87, 0.45)',
      ink: 'rgba(246, 239, 226, 0.92)',
      muted: 'rgba(168, 158, 140, 0.7)'
    };
  }

  // ----- transform backend corpus.graph.json into brain schema -----
  // Backend node:  { id, cluster, title, slug, type, tier, layer, tags, degree }
  // Brain   node:  { id, t (tier code), l (label), g (group), degree }
  // Backend cluster → brain tier code mapping
  const CLUSTER_TO_TIER = {
    'principles-wikis':   'pw',
    'principles-sources': 'ps',
    'knowledge-wikis':    'kw',
    'knowledge-sources':  'ks',
  };
  // Cap node count for visual readability — 340×340 widget can show ~60
  // before edges become dense fog. Keep highest-degree nodes (most central).
  // Bump later if widget grows / view zooms.
  const BRAIN_NODE_CAP = 60;

  function transformBackendGraph(graph) {
    if (!graph || !Array.isArray(graph.nodes)) return null;
    const ranked = graph.nodes.slice().sort((a, b) => (b.degree || 0) - (a.degree || 0));
    const top = ranked.slice(0, BRAIN_NODE_CAP);
    const keep = new Set(top.map(n => n.id));

    const nodes = top.map(n => {
      const segs = n.id.split('/');
      // wikilink path like `wikis/principles/concepts/foo` → group=concepts
      const group = segs.length >= 3 ? segs[segs.length - 2] : undefined;
      return {
        id: n.id,
        t: CLUSTER_TO_TIER[n.cluster] || 'pw',
        l: n.title || segs[segs.length - 1] || n.id,
        g: group,
        degree: n.degree || 0,
      };
    });
    const edges = (graph.edges || [])
      .filter(e => keep.has(e.from) && keep.has(e.to))
      .map(e => [e.from, e.to]);
    return { nodes, edges };
  }

  // ----- mount -----
  function mount(host, opts) {
    opts = opts || {};
    const W = host.clientWidth, H = host.clientHeight;

    // Use real corpus graph when provided by caller (BrainWidget fetches
    // /api/v1/corpus/graph and passes it through), otherwise fall back to
    // the bundled fixture so the widget stays alive on a cold checkout.
    let activeNodes = NODES;
    let activeEdges = EDGES;
    if (opts.graph) {
      const transformed = transformBackendGraph(opts.graph);
      if (transformed && transformed.nodes.length > 0) {
        activeNodes = transformed.nodes;
        activeEdges = transformed.edges;
      }
    }

    const cvs = document.createElement('canvas');
    cvs.width  = W * devicePixelRatio;
    cvs.height = H * devicePixelRatio;
    cvs.style.width  = W + 'px';
    cvs.style.height = H + 'px';
    host.appendChild(cvs);
    const ctx = cvs.getContext('2d');
    ctx.scale(devicePixelRatio, devicePixelRatio);

    // ID lookup
    const byId = {};
    activeNodes.forEach(n => byId[n.id] = n);

    // node neighbor counts → size
    const degree = {};
    activeEdges.forEach(([a, b]) => {
      degree[a] = (degree[a] || 0) + 1;
      degree[b] = (degree[b] || 0) + 1;
    });

    // initial node placement: 4 quadrants by tier
    const cx = W / 2, cy = H / 2;
    const quadCenter = {
      pw: { x: cx - W * 0.18, y: cy - H * 0.16 },  // top-left
      ps: { x: cx - W * 0.22, y: cy + H * 0.18 },  // bottom-left
      kw: { x: cx + W * 0.18, y: cy - H * 0.16 },  // top-right
      ks: { x: cx + W * 0.22, y: cy + H * 0.18 }   // bottom-right
    };

    const sim = activeNodes.map(n => {
      const c = quadCenter[n.t];
      return {
        ref: n,
        x: c.x + (Math.random() - 0.5) * 60,
        y: c.y + (Math.random() - 0.5) * 60,
        vx: 0, vy: 0,
        r: 1.6 + Math.min(degree[n.id] || 1, 8) * 0.45,
        deg: degree[n.id] || 1
      };
    });
    const simById = {};
    sim.forEach(s => simById[s.ref.id] = s);

    // edges as references to sim nodes
    const edges = activeEdges.map(([a, b]) => ({ a: simById[a], b: simById[b] })).filter(e => e.a && e.b);

    // pulses — randomly fire along an edge to suggest "neuron firing"
    const pulses = [];
    function maybeFire() {
      // 1) deliberate firing if external set
      if (state.fireQueue.length) {
        const id = state.fireQueue.shift();
        const node = simById[id];
        if (node) {
          const adj = edges.filter(e => e.a === node || e.b === node);
          adj.slice(0, 4).forEach(e => pulses.push({ e, t: 0, dur: 900 + Math.random() * 400, focal: true }));
          state.activeNode = node;
          state.activeUntil = performance.now() + 1800;
        }
      }
      // 2) ambient
      if (pulses.length < 6 && Math.random() < 0.04) {
        const e = edges[Math.floor(Math.random() * edges.length)];
        pulses.push({ e, t: 0, dur: 1200 + Math.random() * 800 });
      }
    }

    const state = {
      hover: null,
      fireQueue: [],
      activeNode: null,
      activeUntil: 0,
      tStart: performance.now(),
      // physics on/off
      cooling: 1
    };

    // physics step
    function step(dt) {
      // forces
      const cool = state.cooling;
      // repulsion (n^2 but n ~60, fine)
      for (let i = 0; i < sim.length; i++) {
        const a = sim[i];
        for (let j = i + 1; j < sim.length; j++) {
          const b = sim[j];
          let dx = a.x - b.x, dy = a.y - b.y;
          let d2 = dx * dx + dy * dy;
          if (d2 < 0.01) { d2 = 0.01; dx = (Math.random() - 0.5); dy = (Math.random() - 0.5); }
          const d = Math.sqrt(d2);
          const f = 320 / d2;
          const fx = (dx / d) * f, fy = (dy / d) * f;
          a.vx += fx; a.vy += fy;
          b.vx -= fx; b.vy -= fy;
        }
      }
      // edge spring (target length depends on whether cross-tier)
      edges.forEach(e => {
        const a = e.a, b = e.b;
        const target = (a.ref.t === b.ref.t) ? 26 : 50;
        const dx = b.x - a.x, dy = b.y - a.y;
        const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
        const f = (d - target) * 0.018;
        const fx = (dx / d) * f, fy = (dy / d) * f;
        a.vx += fx; a.vy += fy;
        b.vx -= fx; b.vy -= fy;
      });
      // quadrant pull (very gentle — keeps clusters in their corner)
      sim.forEach(s => {
        const c = quadCenter[s.ref.t];
        s.vx += (c.x - s.x) * 0.0024;
        s.vy += (c.y - s.y) * 0.0024;
      });
      // global center pull
      sim.forEach(s => {
        s.vx += (cx - s.x) * 0.0005;
        s.vy += (cy - s.y) * 0.0005;
      });
      // damping + integrate
      const damp = 0.85;
      sim.forEach(s => {
        s.vx *= damp; s.vy *= damp;
        s.x += s.vx * cool;
        s.y += s.vy * cool;
        // bounds
        const margin = 14;
        if (s.x < margin)     { s.x = margin;     s.vx *= -0.4; }
        if (s.x > W - margin) { s.x = W - margin; s.vx *= -0.4; }
        if (s.y < margin)     { s.y = margin;     s.vy *= -0.4; }
        if (s.y > H - margin) { s.y = H - margin; s.vy *= -0.4; }
      });
      // gradual cooling so it settles but never quite stops (drifts forever)
      if (state.cooling > 0.18) state.cooling *= 0.997;
    }

    // render
    function draw(now) {
      const C = colors();
      ctx.clearRect(0, 0, W, H);

      // glow vignette in center
      const grad = ctx.createRadialGradient(cx, cy, 6, cx, cy, W * 0.55);
      grad.addColorStop(0, 'rgba(217, 119, 87, 0.07)');
      grad.addColorStop(1, 'rgba(217, 119, 87, 0)');
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, W, H);

      // edges
      edges.forEach(e => {
        const isActive = state.activeNode && (e.a === state.activeNode || e.b === state.activeNode);
        ctx.strokeStyle = isActive ? C.lineHi : C.line;
        ctx.lineWidth = isActive ? 0.9 : 0.5;
        ctx.beginPath();
        ctx.moveTo(e.a.x, e.a.y);
        ctx.lineTo(e.b.x, e.b.y);
        ctx.stroke();
      });

      // pulses
      for (let i = pulses.length - 1; i >= 0; i--) {
        const p = pulses[i];
        p.t += 16;
        const k = p.t / p.dur;
        if (k >= 1) { pulses.splice(i, 1); continue; }
        const x = p.e.a.x + (p.e.b.x - p.e.a.x) * k;
        const y = p.e.a.y + (p.e.b.y - p.e.a.y) * k;
        const baseColor = (p.e.a.ref.t.startsWith('p') || p.e.b.ref.t.startsWith('p')) ? C.pw : C.kw;
        ctx.fillStyle = baseColor;
        ctx.shadowColor = baseColor;
        ctx.shadowBlur = p.focal ? 14 : 8;
        ctx.beginPath();
        ctx.arc(x, y, p.focal ? 2.4 : 1.6, 0, Math.PI * 2);
        ctx.fill();
        ctx.shadowBlur = 0;
      }

      // nodes
      sim.forEach(s => {
        const C2 = colors();
        const col = C2[s.ref.t];
        const isActive = (state.activeNode === s) && (now < state.activeUntil);
        const isHover  = state.hover === s;
        const breathe = 1 + Math.sin((now - state.tStart) * 0.001 + s.x * 0.01) * 0.06;
        const r = s.r * breathe * (isActive ? 1.7 : (isHover ? 1.4 : 1));

        if (isActive || isHover) {
          ctx.shadowColor = col;
          ctx.shadowBlur = 14;
        }
        ctx.fillStyle = col;
        ctx.beginPath();
        ctx.arc(s.x, s.y, r, 0, Math.PI * 2);
        ctx.fill();
        ctx.shadowBlur = 0;

        // outer ring on active
        if (isActive) {
          ctx.strokeStyle = col;
          ctx.lineWidth = 0.6;
          ctx.globalAlpha = 0.5;
          ctx.beginPath();
          ctx.arc(s.x, s.y, r + 4, 0, Math.PI * 2);
          ctx.stroke();
          ctx.globalAlpha = 1;
        }
      });

      // node label for active node
      if (state.activeNode && now < state.activeUntil) {
        const C2 = colors();
        const s = state.activeNode;
        const text = s.ref.l;
        ctx.font = '9.5px "JetBrains Mono", monospace';
        const w = ctx.measureText(text).width;
        const padX = 5, padY = 3;
        let bx = s.x + 8, by = s.y - 14;
        if (bx + w + padX * 2 > W - 4) bx = s.x - w - padX * 2 - 8;
        if (by < 4) by = s.y + 10;
        ctx.fillStyle = 'rgba(33, 29, 25, 0.95)';
        ctx.strokeStyle = 'rgba(217, 119, 87, 0.4)';
        ctx.lineWidth = 0.5;
        const bw = w + padX * 2, bh = 14;
        roundRect(ctx, bx, by, bw, bh, 3); ctx.fill(); ctx.stroke();
        ctx.fillStyle = C2.pw;
        ctx.fillText(text, bx + padX, by + 10);
      }
    }

    function roundRect(ctx, x, y, w, h, r) {
      ctx.beginPath();
      ctx.moveTo(x + r, y);
      ctx.lineTo(x + w - r, y);
      ctx.quadraticCurveTo(x + w, y, x + w, y + r);
      ctx.lineTo(x + w, y + h - r);
      ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
      ctx.lineTo(x + r, y + h);
      ctx.quadraticCurveTo(x, y + h, x, y + h - r);
      ctx.lineTo(x, y + r);
      ctx.quadraticCurveTo(x, y, x + r, y);
    }

    // hover detection
    cvs.addEventListener('mousemove', (ev) => {
      const r = cvs.getBoundingClientRect();
      const mx = ev.clientX - r.left, my = ev.clientY - r.top;
      let best = null, bd = 100;
      sim.forEach(s => {
        const d = Math.hypot(s.x - mx, s.y - my);
        if (d < 8 && d < bd) { bd = d; best = s; }
      });
      state.hover = best;
      cvs.style.cursor = best ? 'pointer' : 'default';
    });
    cvs.addEventListener('mouseleave', () => { state.hover = null; });

    // raf loop
    let last = performance.now();
    function loop(now) {
      const dt = Math.min(40, now - last);
      last = now;
      step(dt);
      maybeFire();
      draw(now);
      requestAnimationFrame(loop);
    }
    requestAnimationFrame(loop);

    return {
      // public: light up specific concepts as the agent reasons
      fire(ids) {
        (ids || []).forEach(id => state.fireQueue.push(id));
        state.cooling = Math.max(state.cooling, 0.6);
      },
      stats() {
        return {
          nodes: sim.length,
          edges: edges.length,
          tiers: ['pw', 'ps', 'kw', 'ks'].map(t => ({ t, n: sim.filter(s => s.ref.t === t).length }))
        };
      }
    };
  }

  window.LeekBrain = { mount, NODES, EDGES };
})();
