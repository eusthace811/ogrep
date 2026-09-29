// SVG charts, drawn by hand: a mirrored histogram over time (lines added
// above the axis, removed below) and small line charts for evaluation trends.

import { s, h } from "./lib.js";

const SERIES = ["--s1", "--s5", "--s8", "--s7", "--s6", "--s2", "--s9"];
export const seriesColor = (i) => `var(${SERIES[i % SERIES.length]})`;

function niceMax(v) {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

/// buckets: [{t0, t1, up, down, label}] in time order.
export function mirrored(buckets, { width = 900, height = 150, onPick, upLabel = "added", downLabel = "removed" } = {}) {
  const pad = { l: 34, r: 8, t: 8, b: 20 };
  const iw = width - pad.l - pad.r, ih = height - pad.t - pad.b;
  const maxUp = niceMax(Math.max(0, ...buckets.map((b) => b.up)));
  const maxDown = niceMax(Math.max(0, ...buckets.map((b) => b.down)));
  const total = maxUp + maxDown || 1;
  const zero = pad.t + (ih * maxUp) / total;
  const bw = iw / Math.max(1, buckets.length);
  const svg = s("svg", { class: "chart", viewBox: `0 0 ${width} ${height}`, width: "100%", role: "img", "aria-label": `Matching lines ${upLabel} and ${downLabel} over time` });
  svg.appendChild(s("line", { class: "axis", x1: pad.l, x2: width - pad.r, y1: zero, y2: zero }));
  svg.appendChild(s("text", { x: pad.l - 6, y: pad.t + 8, "text-anchor": "end" }, maxUp));
  svg.appendChild(s("text", { x: pad.l - 6, y: height - pad.b, "text-anchor": "end" }, maxDown));
  let lastYear = null;
  buckets.forEach((b, i) => {
    const x = pad.l + i * bw;
    const hu = ((zero - pad.t) * b.up) / maxUp || 0;
    const hd = ((pad.t + ih - zero) * b.down) / maxDown || 0;
    if (b.up) svg.appendChild(s("rect", { class: "up", x: x + 0.5, y: zero - hu, width: Math.max(1, bw - 1), height: hu }));
    if (b.down) svg.appendChild(s("rect", { class: "down", x: x + 0.5, y: zero, width: Math.max(1, bw - 1), height: hd }));
    const year = new Date(b.t0 * 1000).getUTCFullYear();
    if (year !== lastYear && (lastYear === null || bw * 12 > 24 || year % 5 === 0)) {
      svg.appendChild(s("line", { class: "gridline", x1: x, x2: x, y1: pad.t, y2: height - pad.b }));
      svg.appendChild(s("text", { x: x + 2, y: height - 6 }, year));
      lastYear = year;
    }
    const hit = s("rect", { class: "hover", x, y: pad.t, width: bw, height: ih, onclick: () => onPick?.(b) },
      s("title", null, `${b.label}: ${b.up} ${upLabel}, ${b.down} ${downLabel}${b.commits ? `, ${b.commits} commits` : ""}`));
    svg.appendChild(hit);
  });
  return svg;
}

/// A small line chart. series: [{name, points: [{x, y, label}]}], x shared.
export function lines(series, { width = 260, height = 110, format = String, higherIsBetter = true } = {}) {
  const pad = { l: 40, r: 8, t: 8, b: 16 };
  const iw = width - pad.l - pad.r, ih = height - pad.t - pad.b;
  const all = series.flatMap((se) => se.points);
  const xs = [...new Set(all.map((p) => p.x))].sort((a, b) => a - b);
  let lo = Math.min(...all.map((p) => p.y)), hi = Math.max(...all.map((p) => p.y));
  if (lo === hi) { lo -= Math.abs(lo) * 0.1 || 1; hi += Math.abs(hi) * 0.1 || 1; }
  const span = hi - lo;
  lo -= span * 0.08; hi += span * 0.08;
  const X = (x) => pad.l + (xs.length === 1 ? iw / 2 : (xs.indexOf(x) / (xs.length - 1)) * iw);
  const Y = (y) => pad.t + ih - ((y - lo) / (hi - lo)) * ih;
  const svg = s("svg", { class: "chart", viewBox: `0 0 ${width} ${height}`, width: "100%" });
  for (const v of [lo + (hi - lo) * 0.1, lo + (hi - lo) * 0.9]) {
    svg.appendChild(s("line", { class: "gridline", x1: pad.l, x2: width - pad.r, y1: Y(v), y2: Y(v) }));
    svg.appendChild(s("text", { x: pad.l - 4, y: Y(v) + 3, "text-anchor": "end" }, format(v)));
  }
  svg.appendChild(s("line", { class: "axis", x1: pad.l, x2: width - pad.r, y1: pad.t + ih, y2: pad.t + ih }));
  svg.appendChild(s("text", { x: pad.l, y: height - 3 }, "older"));
  svg.appendChild(s("text", { x: width - pad.r, y: height - 3, "text-anchor": "end" }, higherIsBetter ? "newer · higher is better" : "newer · lower is better"));
  series.forEach((se, i) => {
    const pts = [...se.points].sort((a, b) => a.x - b.x);
    const d = pts.map((p, j) => `${j ? "L" : "M"}${X(p.x).toFixed(1)},${Y(p.y).toFixed(1)}`).join("");
    svg.appendChild(s("path", { class: "series", d, stroke: seriesColor(i) }));
    for (const p of pts) {
      svg.appendChild(s("circle", { class: "pt", cx: X(p.x), cy: Y(p.y), r: 3, fill: seriesColor(i) },
        s("title", null, `${se.name}: ${format(p.y)}\n${p.label || ""}`)));
    }
  });
  return svg;
}

export function legend(names) {
  return h("div", { class: "legend" }, names.map((name, i) => h("span", null, h("i", { style: { background: seriesColor(i) } }), name)));
}
