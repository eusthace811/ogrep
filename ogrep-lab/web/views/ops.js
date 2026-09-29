// Ops: services and what they depend on, from the repository's operations
// files; the blast radius of one; likely causes when several fail.

import { h, clear, api, aborted, n, date, short, href, replace, loading, errorBox, empty, table } from "../lib.js";

const SINCE = [["1h", "1 hour"], ["6h", "6 hours"], ["24h", "24 hours"], ["3d", "3 days"], ["7d", "7 days"], ["30d", "30 days"], ["1y", "1 year"]];

let els = {};
let entities = null;
let st = { entity: "", failing: "", since: "24h" };

function evidence(ev) {
  return h("a", { class: "mono small", href: href("files", { path: ev.file, line: ev.line }) }, `${ev.file}:${ev.line}`);
}

function sinceSelect(value, onchange) {
  const el = h("select", { class: "field", "aria-label": "Since", onchange: () => onchange(el.value) },
    SINCE.map(([v, label]) => h("option", { value: v }, `last ${label}`)));
  el.value = value;
  return el;
}

function sync() {
  replace("ops", st);
}

async function loadEntities() {
  clear(els.list, loading());
  try {
    const r = await api("feature", { mode: "entities", q: ".", limit: 5000 });
    entities = r.records.filter((x) => x.type === "entity").map((x) => x.data);
    if (!entities.length) {
      clear(els.list, empty(
        "No services or other entities found. ogrep reads Docker Compose, Kubernetes manifests, Helm charts, Backstage catalog files, Prometheus rules and CODEOWNERS, plus an ",
        h("code", null, "ogrep-ontology.yaml"), " at the repository root for anything else."));
      return;
    }
    drawList("");
  } catch (e) {
    if (!aborted(e)) clear(els.list, errorBox(e));
  }
}

function drawList(filter) {
  const f = filter.toLowerCase();
  const rows = entities.filter((e) => !f || e.entity.id.toLowerCase().includes(f) || e.entity.kind.includes(f)).map((e) => {
    const deps = [...new Set(e.edges.filter((x) => x.relation !== "telemetry").map((x) => x.to))].sort();
    return { id: e.entity.id, kind: e.entity.kind, deps };
  });
  clear(els.list, table([
    { key: "id", label: "Entity", render: (r) => h("span", { class: "mono", style: { whiteSpace: "nowrap" } }, r.id) },
    { key: "kind", label: "Kind", render: (r) => h("span", { class: "muted" }, r.kind) },
    { key: "deps", label: "Depends on", sort: (r) => r.deps.length, render: (r) => h("span", { class: "small muted" }, r.deps.join(", ") || "–") },
  ], rows, { onclick: (r) => { st.entity = r.id; sync(); showEntity(); }, selected: (r) => r.id === st.entity, initial: { key: "id", dir: 1 } }));
}

async function showEntity() {
  if (entities) drawList(els.filter.value);
  const e = entities?.find((x) => x.entity.id === st.entity);
  if (!e) { clear(els.detail, h("p", { class: "faint" }, "Select an entity to see what it depends on, what depends on it, and recent changes.")); return; }
  const blast = h("div", null, loading());
  const timeline = h("div", null, loading());
  clear(els.detail,
    h("h2", null, e.entity.id, " ", h("span", { class: "pill" }, e.entity.kind)),
    h("div", { class: "small" }, (e.entity.locations || []).map((l) => h("div", null, "defined at ", evidence(l)))),
    h("h3", null, "Depends on"),
    e.edges.length ? h("table", { class: "grid" }, h("tbody", null, e.edges.map((x) => h("tr", null,
      h("td", { class: "mono" }, x.to),
      h("td", { class: "muted small" }, x.relation, x.provenance === "inferred" ? h("span", { class: "faint" }, " (inferred)") : null),
      h("td", null, evidence(x.evidence)),
      h("td", { class: "mono small faint" }, x.text))))) : h("p", { class: "faint small" }, "Nothing."),
    h("h3", null, "Blast radius"),
    h("p", { class: "faint small" }, "Everything that depends on it, directly or not. Telemetry links are left out: otherwise every service reaches every other through the collector."),
    blast,
    h("h3", null, "Recent changes to it and its dependencies"),
    h("div", { class: "querybar" }, sinceSelect(st.since, (v) => { st.since = v; els.since.value = v; sync(); showEntity(); })),
    timeline);
  try {
    const r = await api("feature", { mode: "blast", q: e.entity.id }, { slot: "blast" });
    const rows = r.records.filter((x) => x.type === "blast").map((x) => x.data);
    const byDepth = new Map();
    for (const row of rows) { if (!byDepth.has(row.depth)) byDepth.set(row.depth, []); byDepth.get(row.depth).push(row); }
    clear(blast, rows.length
      ? h("table", { class: "grid" }, h("tbody", null, [...byDepth.entries()].flatMap(([d, list]) => [
          h("tr", null, h("td", { colspan: "3", class: "label", style: { paddingTop: "10px" } }, d === 1 ? "Directly" : `${d} steps away`)),
          ...list.map((x) => h("tr", { class: "clickable", onclick: () => { st.entity = x.service; sync(); showEntity(); } },
            h("td", { class: "mono", style: { width: "30%" } }, x.service),
            h("td", { class: "muted small", style: { width: "30%" } }, `${x.edge.relation} ${x.edge.to}`),
            h("td", null, evidence(x.edge.evidence)))),
        ])))
      : h("p", { class: "faint small" }, "Nothing depends on it."));
  } catch (err) {
    if (!aborted(err)) clear(blast, errorBox(err));
  }
  try {
    const r = await api("feature", { mode: "timeline", q: e.entity.id, since: st.since }, { slot: "timeline" });
    const rows = r.records.filter((x) => x.type === "event").map((x) => x.data);
    clear(timeline, rows.length
      ? h("table", { class: "grid" }, h("tbody", null, rows.map((x) => h("tr", null,
          h("td", { class: "small faint", style: { whiteSpace: "nowrap" } }, date(x.time)),
          h("td", { class: "mono small" }, x.service),
          h("td", null, x.commit.subject, h("div", { class: "faint small mono" }, short(x.commit.sha), "  ", x.commit.files.slice(0, 4).join("  "), x.commit.files.length > 4 ? ` +${x.commit.files.length - 4}` : ""))))))
      : h("p", { class: "faint small" }, "No commits in this period."));
  } catch (err) {
    if (!aborted(err)) clear(timeline, errorBox(err));
  }
}

async function suspects() {
  const failing = els.failing.value.trim();
  st.failing = failing;
  sync();
  if (!failing) { clear(els.suspects); return; }
  clear(els.suspects, loading("Ranking"));
  try {
    const r = await api("feature", { mode: "suspects", q: failing, since: st.since }, { slot: "suspects" });
    const rows = r.records.filter((x) => x.type === "suspect").map((x) => x.data);
    const max = rows[0]?.suspect.score || 1;
    clear(els.suspects, rows.length ? h("table", { class: "grid" },
      h("thead", null, h("tr", null, h("th", { class: "num" }, "#"), h("th", null, "Suspect"), h("th", null, "Score"), h("th", null, "Explains"), h("th", { class: "num" }, "Commits"), h("th", null, "Latest change"))),
      h("tbody", null, rows.map((x) => {
        const s = x.suspect;
        const latest = s.changes?.[0];
        return h("tr", { class: "clickable", onclick: () => { st.entity = s.service; sync(); showEntity(); } },
          h("td", { class: "num faint" }, x.rank),
          h("td", { class: "mono" }, s.service),
          h("td", null, h("span", { class: "score" }, h("span", { class: "meter" }, h("i", { style: { width: `${(s.score / max) * 100}%` } })), s.score.toFixed(2))),
          h("td", { class: "small muted" }, s.explains.join(", ")),
          h("td", { class: "num" }, n(s.changes?.length || 0)),
          h("td", { class: "small" }, latest ? [h("span", { class: "faint" }, `${date(latest.time)} `), latest.subject] : h("span", { class: "faint" }, "none in the period")));
      }))) : empty("No suspects: none of these names is a known service, or they depend on nothing."));
  } catch (e) {
    if (!aborted(e)) clear(els.suspects, errorBox(e));
  }
}

function mount(root) {
  const filter = h("input", { class: "field", placeholder: "Filter entities", "aria-label": "Filter entities", "data-primary": "", style: { width: "100%" },
    oninput: () => entities && drawList(filter.value) });
  const failing = h("input", { class: "field query", placeholder: "Failing services, comma-separated: checkout, frontend", "aria-label": "Failing services",
    onkeydown: (e) => { if (e.key === "Enter") suspects(); } });
  els = {
    filter, failing,
    since: sinceSelect(st.since, (v) => { st.since = v; suspects(); showEntity(); }),
    list: h("div", { style: { marginTop: "8px" } }),
    detail: h("div"),
    suspects: h("div"),
  };
  clear(root, h("div", { class: "split rail-left", style: { gridTemplateColumns: "380px minmax(0, 1fr)" } },
    h("nav", { class: "rail left", "aria-label": "Entities" }, filter, els.list),
    h("div", { class: "pane" },
      h("h2", null, "Incident suspects"),
      h("p", { class: "prose small" }, "Services the failing ones depend on, weighted by how many failures each explains and by commits in the period."),
      h("div", { class: "querybar" }, failing, els.since, h("button", { class: "btn primary", type: "button", onclick: suspects }, "Rank")),
      els.suspects,
      h("div", { class: "section" }, els.detail))));
}

function route(p) {
  // A bare tab link returns to the view as it was left.
  if (!Object.keys(p).length && (st.entity || st.failing)) sync();
  else st = { entity: p.entity || "", failing: p.failing || "", since: p.since || "24h" };
  els.failing.value = st.failing;
  // A period not in the list (from a hand-edited URL) is added so it shows.
  if (![...els.since.options].some((o) => o.value === st.since)) els.since.appendChild(h("option", { value: st.since }, `since ${st.since}`));
  els.since.value = st.since;
  const first = entities === null;
  (first ? loadEntities() : Promise.resolve()).then(() => {
    showEntity();
    if (st.failing && first) suspects();
  });
}

export default { id: "ops", title: "Ops", mount, route };
