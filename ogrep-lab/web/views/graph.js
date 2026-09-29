// Graph: the neighbourhood of a file in the file graph, entered from the most
// central files or a definition search.

import { h, clear, api, aborted, n, href, pathLink, go, replace, errorBox, empty, segmented, debounce, state } from "../lib.js";
import { ForceGraph } from "../graph.js";

let els = {};
let fg = null;
let current = { path: null, hops: "1", per: "12" };
let data = null;
let tip = null;

function basename(p) {
  return p.slice(p.lastIndexOf("/") + 1);
}

function showTip(node, ev) {
  if (!node) { tip?.remove(); tip = null; return; }
  if (!tip) { tip = h("div", { class: "tooltip" }); document.body.appendChild(tip); }
  const d = data.nodes[node.index];
  tip.textContent = `${d.path}\n${n(d.defs)} definitions · uses ${n(d.out)} · used by ${n(d.in)}\nPageRank ${d.pagerank.toExponential(2)}`;
  tip.style.left = `${ev.clientX + 14}px`;
  tip.style.top = `${ev.clientY + 12}px`;
}

function details(i) {
  const d = data.nodes[i];
  const outs = data.edges.filter((e) => e.from === i).sort((a, b) => b.weight - a.weight);
  const ins = data.edges.filter((e) => e.to === i).sort((a, b) => b.weight - a.weight);
  const row = (j, w) => h("div", { class: "item" },
    pathLink(data.nodes[j].path, href("graph", { path: data.nodes[j].path, hops: current.hops, per: current.per }), w.toFixed(1)));
  clear(els.detail,
    h("h2", null, basename(d.path)),
    h("p", { class: "mono small muted", style: { overflowWrap: "anywhere" } }, d.path),
    h("dl", { class: "dl small" },
      h("dt", null, "Definitions"), h("dd", null, n(d.defs)),
      h("dt", null, "Uses"), h("dd", null, `${n(d.out)} files`),
      h("dt", null, "Used by"), h("dd", null, `${n(d.in)} files`),
      h("dt", null, "PageRank"), h("dd", null, d.pagerank.toExponential(3))),
    h("p", { class: "small" },
      h("a", { href: href("files", { path: d.path }) }, "Open file"), "  ·  ",
      i !== 0 ? h("a", { href: href("graph", { path: d.path, hops: current.hops, per: current.per }) }, "Centre here") : h("span", { class: "faint" }, "centre"), "  ·  ",
      h("a", { href: href("search", { q: basename(d.path).replace(/\.[^.]*$/, ""), fixed: "1", lens: "near" }) }, "Related by name")),
    h("h3", null, "Uses, among these"),
    outs.length ? h("div", { class: "links" }, outs.map((e) => row(e.to, e.weight))) : h("p", { class: "faint small" }, "None shown."),
    h("h3", null, "Used by, among these"),
    ins.length ? h("div", { class: "links" }, ins.map((e) => row(e.from, e.weight))) : h("p", { class: "faint small" }, "None shown."));
}

async function load() {
  if (!current.path) return;
  clear(els.status, h("span", { class: "loading" }, "Loading"));
  try {
    data = await api("graph", { path: current.path, hops: current.hops, per: current.per }, { slot: "graph" });
    const maxPr = Math.max(...data.nodes.map((d) => d.pagerank));
    const outOfCenter = new Set(data.edges.filter((e) => e.from === 0).map((e) => e.to));
    const intoCenter = new Set(data.edges.filter((e) => e.to === 0).map((e) => e.from));
    const nodes = data.nodes.map((d, i) => ({
      index: i,
      label: basename(d.path),
      depth: d.depth,
      r: 3.5 + 9 * Math.sqrt(d.pagerank / maxPr),
      kind: i === 0 ? "center" : outOfCenter.has(i) && intoCenter.has(i) ? "both" : outOfCenter.has(i) ? "uses" : intoCenter.has(i) ? "usedby" : "far",
    }));
    fg.userMoved = false;
    fg.setData(nodes, data.edges);
    fg.nodes[0].selected = true;
    details(0);
    clear(els.status,
      h("span", null, h("b", null, n(data.nodes.length)), " files, ", h("b", null, n(data.edges.length)), " links"),
      data.current ? null : h("span", { class: "accent" }, "graph older than the index"));
  } catch (e) {
    if (aborted(e)) return;
    // Do not leave the previous graph up under an error, and say it where it
    // shows at every width (the detail rail is hidden on narrow screens).
    data = null;
    fg.setData([], []);
    clear(els.status, h("span", { class: "accent" }, e.message));
    clear(els.detail, errorBox(e));
  }
}

async function loadCentral() {
  try {
    const c = await api("central", { limit: 60 });
    const max = c.files[0]?.pagerank || 1;
    clear(els.central, h("div", { class: "links" }, c.files.map((f) => h("div", { class: "item" },
      h("span", { class: "w", title: `PageRank ${f.pagerank.toExponential(2)}` }, h("i", { style: { width: `${(f.pagerank / max) * 100}%` } })),
      pathLink(f.path, href("graph", { path: f.path, hops: current.hops, per: current.per }), `used by ${n(f.in)}`)))));
  } catch (e) {
    if (!aborted(e)) clear(els.central, errorBox(e));
  }
}

const findSymbols = debounce(async (q) => {
  if (q.trim().length < 2) { els.symbols.hidden = true; els.central.hidden = false; return; }
  try {
    const r = await api("symbols", { q: q.trim(), limit: 60 }, { slot: "graph-symbols" });
    els.central.hidden = true;
    els.symbols.hidden = false;
    clear(els.symbols,
      h("p", { class: "faint small" }, `${n(r.total)} definitions`),
      h("div", { class: "links" }, r.symbols.map((s) => h("div", { class: "item" },
        h("a", { class: "mono small", href: href("graph", { path: s.path, hops: current.hops, per: current.per }) }, s.name),
        h("span", { class: "faint small" }, ` ${s.kind}`),
        h("div", { class: "via" }, `${s.path}:${s.line}`)))));
  } catch (e) {
    if (!aborted(e)) clear(els.symbols, errorBox(e));
  }
}, 180);

function mount(root) {
  const canvas = h("canvas", { "aria-label": "File graph; the list on the right has the same content" });
  const search = h("input", { class: "field", placeholder: "Find a definition", "aria-label": "Find a definition", "data-primary": "", spellcheck: "false",
    style: { width: "100%" }, oninput: () => findSymbols(search.value) });
  const hops = segmented([{ value: "1", label: "1 hop" }, { value: "2", label: "2 hops" }], current.hops, (v) => { current.hops = v; replace("graph", current); load(); });
  const per = segmented([{ value: "8", label: "8" }, { value: "12", label: "12" }, { value: "24", label: "24" }], current.per, (v) => { current.per = v; replace("graph", current); load(); });
  els = {
    hops, per,
    central: h("div"),
    symbols: h("div", { hidden: true }),
    detail: h("div"),
    status: h("div", { class: "statusline", style: { margin: 0 } }),
  };
  const legend = h("div", { class: "canvas-legend" },
    h("span", null, h("i", { style: { background: "var(--accent)" } }), "this file"),
    h("span", null, h("i", { style: { background: "var(--ink-2)" } }), "it uses"),
    h("span", null, h("i", { style: { border: "1.5px solid var(--ink-2)", background: "var(--surface)" } }), "uses it"),
    h("span", null, h("i", { style: { background: "var(--ink-2)", boxShadow: "0 0 0 1.5px var(--surface), 0 0 0 2.5px var(--ink-2)" } }), "both"),
    h("span", null, "size: PageRank · drag, scroll to zoom, double-click to open"));
  els.wrap = h("div", { class: "canvas-wrap" }, canvas, legend,
    h("div", { class: "canvas-tools" }, els.status, hops, per));
  clear(root, h("div", { class: "split three" },
    h("nav", { class: "rail left", "aria-label": "Entry points" },
      search, els.symbols,
      h("div", null, h("h2", { style: { marginTop: "14px" } }, "Most central files"),
        h("p", { class: "faint small" }, "By PageRank over the links: what the rest of the code leans on."), els.central)),
    els.wrap,
    h("aside", { class: "rail", "aria-label": "Selected file" }, els.detail)));
  fg = new ForceGraph(canvas, {
    onSelect: (node) => { fg.nodes.forEach((x) => (x.selected = x === node)); fg.draw(); details(node.index); },
    onOpen: (node) => go("files", { path: data.nodes[node.index].path }),
    onHover: showTip,
  });
}

let centralLoaded = false;
async function route(p) {
  const o = state.overview || (await api("overview").catch(() => null));
  if (o && !o.graph) {
    clear(els.wrap, h("div", { class: "pane" }, empty("No file graph. Build it with ", h("code", null, "ogrep index --graph"), "; it records definitions and which files use them.")));
    return;
  }
  if (!centralLoaded) { centralLoaded = true; loadCentral(); }
  if (!Object.keys(p).length && current.path) { replace("graph", current); return; }
  const next = { path: p.path || null, hops: p.hops || "1", per: p.per || "12" };
  const changed = JSON.stringify(next) !== JSON.stringify(current) || !data;
  current = next;
  els.hops.set(current.hops);
  els.per.set(current.per);
  if (!current.path) {
    // Start from the most central file rather than an empty canvas.
    const c = await api("central", { limit: 1 }).catch(() => null);
    if (c?.files?.[0]) { current.path = c.files[0].path; replace("graph", current); }
    else { clear(els.detail, empty("Pick a file on the left.")); return; }
  }
  if (changed) load();
}

export default { id: "graph", title: "Graph", mount, route };
