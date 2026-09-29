// A small force-directed graph on a canvas, for neighbourhoods of up to a
// few hundred nodes: repulsion between every pair, springs along links, a
// pull to the centre, and cooling until it settles. Pan with drag, zoom with
// the wheel, hover for details, click to select, double-click to open.

function css(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

export class ForceGraph {
  constructor(canvas, { onSelect, onOpen, onHover } = {}) {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");
    this.onSelect = onSelect;
    this.onOpen = onOpen;
    this.onHover = onHover;
    this.nodes = [];
    this.edges = [];
    this.view = { x: 0, y: 0, k: 1 };
    this.hover = null;
    this.alpha = 0;
    this.frame = null;
    this.bind();
    this.resize = this.resize.bind(this);
    new ResizeObserver(this.resize).observe(canvas);
    window.addEventListener("themechange", () => this.draw());
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => this.draw());
  }

  /// nodes: [{id, label, r, kind}], edges: [{from, to, weight}] by index.
  setData(nodes, edges) {
    const w = this.canvas.clientWidth, hgt = this.canvas.clientHeight;
    // Seed positions on a spiral: deterministic, and far enough apart that
    // the first steps do not explode.
    this.nodes = nodes.map((n, i) => {
      const a = i * 2.399963, rad = i === 0 ? 0 : 30 + 22 * Math.sqrt(i);
      return { ...n, x: Math.cos(a) * rad, y: Math.sin(a) * rad, vx: 0, vy: 0 };
    });
    this.nodes[0] && (this.nodes[0].fixed = true);
    const maxW = Math.max(1e-9, ...edges.map((e) => e.weight));
    this.edges = edges.filter((e) => e.from !== e.to).map((e) => ({ ...e, w: e.weight / maxW }));
    this.adj = this.nodes.map(() => new Set());
    for (const e of this.edges) { this.adj[e.from].add(e.to); this.adj[e.to].add(e.from); }
    this.view = { x: w / 2, y: hgt / 2, k: 1 };
    this.alpha = 1;
    this.start();
  }

  start() {
    if (this.frame) return;
    const tick = () => {
      for (let i = 0; i < 3 && this.alpha > 0.005; i++) this.step();
      this.draw();
      this.frame = this.alpha > 0.005 ? requestAnimationFrame(tick) : null;
      if (!this.frame) this.fit();
    };
    this.frame = requestAnimationFrame(tick);
  }

  step() {
    const ns = this.nodes, a = this.alpha;
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const p = ns[i], q = ns[j];
        let dx = q.x - p.x, dy = q.y - p.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) { dx = (i % 7) - 3 || 1; dy = (j % 5) - 2 || 1; d2 = dx * dx + dy * dy; }
        const f = (2600 * a) / d2;
        const d = Math.sqrt(d2);
        const fx = (dx / d) * f, fy = (dy / d) * f;
        p.vx -= fx; p.vy -= fy; q.vx += fx; q.vy += fy;
      }
    }
    for (const e of this.edges) {
      const p = ns[e.from], q = ns[e.to];
      const dx = q.x - p.x, dy = q.y - p.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const target = 90 + 70 * (1 - e.w);
      const f = ((d - target) / d) * 0.06 * a * (0.4 + e.w);
      p.vx += dx * f; p.vy += dy * f; q.vx -= dx * f; q.vy -= dy * f;
    }
    for (const n of ns) {
      n.vx -= n.x * 0.0025 * a;
      n.vy -= n.y * 0.0025 * a;
      if (n.fixed || n === this.dragNode) { n.vx = n.vy = 0; continue; }
      n.vx *= 0.6; n.vy *= 0.6;
      n.x += n.vx; n.y += n.vy;
    }
    this.alpha *= 0.985;
  }

  fit() {
    if (!this.nodes.length || this.userMoved) return;
    const w = this.canvas.clientWidth, hgt = this.canvas.clientHeight;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of this.nodes) { x0 = Math.min(x0, n.x); y0 = Math.min(y0, n.y); x1 = Math.max(x1, n.x); y1 = Math.max(y1, n.y); }
    const pad = 48;
    const k = Math.min(2.2, (w - pad * 2) / Math.max(1, x1 - x0), (hgt - pad * 2) / Math.max(1, y1 - y0));
    this.view = { k, x: w / 2 - ((x0 + x1) / 2) * k, y: hgt / 2 - ((y0 + y1) / 2) * k };
    this.draw();
  }

  resize() {
    const dpr = window.devicePixelRatio || 1;
    const w = this.canvas.clientWidth, hgt = this.canvas.clientHeight;
    if (!w || !hgt) return;
    this.canvas.width = Math.round(w * dpr);
    this.canvas.height = Math.round(hgt * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    if (!this.userMoved) this.fit(); else this.draw();
  }

  toWorld(px, py) {
    return { x: (px - this.view.x) / this.view.k, y: (py - this.view.y) / this.view.k };
  }

  nodeAt(px, py) {
    const p = this.toWorld(px, py);
    let best = null, bd = Infinity;
    for (const n of this.nodes) {
      const d = Math.hypot(n.x - p.x, n.y - p.y);
      if (d < n.r + 5 / this.view.k && d < bd) { best = n; bd = d; }
    }
    return best;
  }

  bind() {
    const c = this.canvas;
    let down = null;
    const pos = (e) => { const r = c.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; };
    c.addEventListener("pointerdown", (e) => {
      const p = pos(e);
      const n = this.nodeAt(p.x, p.y);
      down = { ...p, vx: this.view.x, vy: this.view.y, node: n, moved: false };
      if (n) this.dragNode = n;
      c.setPointerCapture(e.pointerId);
      c.classList.add("dragging");
    });
    c.addEventListener("pointermove", (e) => {
      const p = pos(e);
      if (down) {
        const dx = p.x - down.x, dy = p.y - down.y;
        if (Math.abs(dx) + Math.abs(dy) > 3) down.moved = true;
        if (down.node && down.moved) {
          const w = this.toWorld(p.x, p.y);
          down.node.x = w.x; down.node.y = w.y; down.node.fixed = true;
          this.alpha = Math.max(this.alpha, 0.2); this.start();
        } else if (down.moved) {
          this.view.x = down.vx + dx; this.view.y = down.vy + dy; this.userMoved = true; this.draw();
        }
        return;
      }
      const n = this.nodeAt(p.x, p.y);
      if (n !== this.hover) {
        this.hover = n;
        this.draw();
      }
      this.onHover?.(n, e);
    });
    c.addEventListener("pointerup", (e) => {
      c.classList.remove("dragging");
      if (down && !down.moved && down.node) this.onSelect?.(down.node);
      this.dragNode = null;
      down = null;
      c.releasePointerCapture(e.pointerId);
    });
    c.addEventListener("pointerleave", () => { this.hover = null; this.onHover?.(null); this.draw(); });
    c.addEventListener("dblclick", (e) => {
      const p = pos(e);
      const n = this.nodeAt(p.x, p.y);
      if (n) this.onOpen?.(n);
    });
    c.addEventListener("wheel", (e) => {
      e.preventDefault();
      const p = pos(e);
      const k = Math.min(4, Math.max(0.2, this.view.k * Math.exp(-e.deltaY * 0.0015)));
      this.view.x = p.x - ((p.x - this.view.x) * k) / this.view.k;
      this.view.y = p.y - ((p.y - this.view.y) * k) / this.view.k;
      this.view.k = k;
      this.userMoved = true;
      this.draw();
    }, { passive: false });
  }

  draw() {
    const { ctx, canvas } = this;
    const w = canvas.clientWidth, hgt = canvas.clientHeight;
    ctx.clearRect(0, 0, w, hgt);
    if (!this.nodes.length) return;
    const col = {
      edge: css("--ink-3"), ink: css("--ink"), ink2: css("--ink-2"), accent: css("--accent"),
      surface: css("--surface"), rule: css("--rule-strong"), font: css("--ui"),
    };
    const focus = this.hover;
    const near = focus ? this.adj[this.nodes.indexOf(focus)] : null;
    ctx.save();
    ctx.translate(this.view.x, this.view.y);
    ctx.scale(this.view.k, this.view.k);

    for (const e of this.edges) {
      const p = this.nodes[e.from], q = this.nodes[e.to];
      const lit = focus && (p === focus || q === focus);
      ctx.globalAlpha = focus ? (lit ? 0.9 : 0.06) : 0.14 + 0.45 * e.w;
      ctx.strokeStyle = lit ? col.accent : col.edge;
      ctx.lineWidth = (0.6 + 1.6 * e.w) / Math.sqrt(this.view.k);
      ctx.beginPath();
      ctx.moveTo(p.x, p.y);
      ctx.lineTo(q.x, q.y);
      ctx.stroke();
      if (lit || (!focus && e.w > 0.5)) this.arrow(p, q, col, lit);
    }
    ctx.globalAlpha = 1;

    this.nodes.forEach((n, i) => {
      const dim = focus && n !== focus && !near.has(i);
      ctx.globalAlpha = dim ? 0.25 : 1;
      ctx.beginPath();
      ctx.arc(n.x, n.y, n.r, 0, Math.PI * 2);
      if (n.kind === "center") { ctx.fillStyle = col.accent; ctx.fill(); }
      else if (n.kind === "uses") { ctx.fillStyle = col.ink2; ctx.fill(); }
      else if (n.kind === "both") {
        ctx.fillStyle = col.ink2; ctx.fill();
        ctx.lineWidth = 1.2 / this.view.k; ctx.strokeStyle = col.ink2;
        ctx.beginPath(); ctx.arc(n.x, n.y, n.r + 2.5 / this.view.k, 0, Math.PI * 2); ctx.stroke();
      }
      else { ctx.fillStyle = col.surface; ctx.fill(); ctx.lineWidth = 1.5 / this.view.k; ctx.strokeStyle = col.ink2; ctx.stroke(); }
      if (n.selected) { ctx.lineWidth = 2 / this.view.k; ctx.strokeStyle = col.accent; ctx.beginPath(); ctx.arc(n.x, n.y, n.r + 3 / this.view.k, 0, Math.PI * 2); ctx.stroke(); }
    });

    // Labels: every first-ring node, plus whatever is hovered.
    const fs = 11 / this.view.k;
    ctx.font = `${n2(fs)}px ${col.font}`;
    ctx.textBaseline = "middle";
    this.nodes.forEach((n, i) => {
      const show = n === focus || (focus ? near.has(i) : n.depth <= 1 || this.nodes.length < 40);
      if (!show) return;
      ctx.globalAlpha = focus && n !== focus && !near.has(i) ? 0.25 : 1;
      const x = n.x + n.r + 4 / this.view.k;
      const text = n.label;
      ctx.lineWidth = 3 / this.view.k;
      ctx.strokeStyle = col.surface;
      ctx.strokeText(text, x, n.y);
      ctx.fillStyle = n.kind === "center" ? col.accent : col.ink;
      ctx.fillText(text, x, n.y);
    });
    ctx.restore();
    ctx.globalAlpha = 1;
  }

  arrow(p, q, col, lit) {
    const { ctx } = this;
    const dx = q.x - p.x, dy = q.y - p.y;
    const d = Math.hypot(dx, dy) || 1;
    const ux = dx / d, uy = dy / d;
    const tipX = q.x - ux * (q.r + 2), tipY = q.y - uy * (q.r + 2);
    const s = 5 / Math.sqrt(this.view.k);
    ctx.fillStyle = lit ? col.accent : col.edge;
    ctx.beginPath();
    ctx.moveTo(tipX, tipY);
    ctx.lineTo(tipX - ux * s * 1.6 - uy * s * 0.8, tipY - uy * s * 1.6 + ux * s * 0.8);
    ctx.lineTo(tipX - ux * s * 1.6 + uy * s * 0.8, tipY - uy * s * 1.6 - ux * s * 0.8);
    ctx.closePath();
    ctx.fill();
  }
}

function n2(x) {
  return Math.round(x * 100) / 100;
}
