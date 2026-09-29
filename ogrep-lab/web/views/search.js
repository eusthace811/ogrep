// Search: one pattern, several lenses (matches, related files, map, callers,
// callees), with the query plan for the same pattern alongside.

import {
  h, clear, api, aborted, n, ms, pct, compact, pathLabel, marked, href, replace, debounce, copy,
  segmented, toggle, errorBox, empty,
} from "../lib.js";

const LENSES = [
  { id: "matches", label: "Matches" },
  { id: "near", label: "Related files" },
  { id: "map", label: "Map" },
  { id: "callers", label: "Callers" },
  { id: "callees", label: "Callees" },
];

const DEFAULTS = { q: "", lens: "matches", fixed: "", case: "smart", word: "", rank: "", scope: "", kind: "", in: "", glob: "", context: "0" };

let st = { ...DEFAULTS };
let els = {};
let lastKey = null;
let lastPlanKey = null;

/// The matching options, which are all the query plan depends on.
function params() {
  return { q: st.q, fixed: st.fixed === "1", case: st.case, word: st.word === "1" };
}

function syncUrl() {
  const p = {};
  for (const [k, v] of Object.entries(st)) if (v !== DEFAULTS[k]) p[k] = v;
  replace("search", p);
}

// ── Results ──────────────────────────────────────────────

const PROSE = [".md", ".rst", ".txt", ".adoc", ".po", ".html", ".tex"];

function scoreCell(rank, top, path) {
  const f = rank.features || {};
  const why = [];
  if (f.def_lines) why.push(h("span", null, h("b", null, n(f.def_lines)), f.def_lines === 1 ? " definition" : " definitions"));
  if (f.body_lines) why.push(h("span", null, h("b", null, n(f.body_lines)), f.body_lines === 1 ? " matching line" : " matching lines"));
  if (f.path) why.push(h("span", null, "name in path"));
  if (f.prior !== undefined && f.prior !== 1) why.push(h("span", null, "weight ", h("b", null, `×${f.prior}`), f.prior < 1 ? (PROSE.some((e) => path.toLowerCase().endsWith(e)) ? " (prose)" : " (test, vendored or generated)") : ""));
  if (f.patterns > 1) why.push(h("span", null, h("b", null, f.patterns), " patterns"));
  return {
    badge: h("span", { class: "score", title: `score ${rank.score}` },
      h("span", { class: "faint" }, `#${rank.rank}`),
      h("span", { class: "meter" }, h("i", { style: { width: `${Math.min(100, (rank.score / top) * 100)}%` } })),
      rank.score.toFixed(2)),
    why: why.length ? h("div", { class: "why" }, why.flatMap((w, i) => (i ? [" · ", w] : [w]))) : null,
  };
}

function lineRow(path, item) {
  const text = item.text ?? "(binary or non-UTF-8 line)";
  return h("a", { class: `line ${item.kind}`, href: href("files", { path, line: item.line, q: st.q }) },
    h("span", { class: "ln" }, item.line ?? ""),
    h("span", { class: "tx" }, item.kind === "match" ? marked(text, item.spans) : text));
}

function crumb(chain) {
  return h("div", { class: "crumb" }, chain.flatMap((c, i) => [
    i ? h("span", { class: "sep" }, "›") : null,
    h("span", null, `${c.kind} `), h("b", null, c.name),
  ]));
}

function renderMatches(data) {
  const top = Math.max(0, ...data.files.map((f) => f.rank?.score ?? 0)) || 1;
  const blocks = data.files.map((f) => {
    const matches = f.items.filter((i) => i.kind === "match").length;
    const rank = f.rank ? scoreCell(f.rank, top, f.path) : null;
    const body = [];
    let prev = null;
    for (const it of f.items) {
      if (it.kind === "scope") { body.push(crumb(it.chain || [])); prev = null; continue; }
      if (prev !== null && it.line > prev + 1 && st.context !== "0") body.push(h("div", { class: "line gap" }, h("span", { class: "ln" }, "⋮"), h("span", { class: "tx" }, "")));
      body.push(lineRow(f.path, it));
      prev = it.line;
    }
    return h("div", { class: "result" },
      h("div", { class: "result-head" },
        h("span", { class: "path" }, h("a", { href: href("files", { path: f.path, q: st.q }) }, pathLabel(f.path))),
        rank ? rank.badge : null,
        h("span", { class: "count" }, `${matches} ${matches === 1 ? "line" : "lines"}`)),
      rank?.why,
      h("div", { class: "lines" }, body));
  });
  return blocks.length ? blocks : [empty("No matches.")];
}

function renderNear(records) {
  const rows = records.filter((r) => r.type === "near").map((r) => r.data);
  if (!rows.length) return [empty("No related files. The pattern may match nothing, or the files it matches have no links in the graph.")];
  const max = rows[0].score || 1;
  return [
    h("p", { class: "prose small" }, "Files that use, or are used by, the files this pattern matches, strongest first, with the names that link them."),
    h("div", { class: "links" }, rows.map((r) => h("div", { class: "item" },
      h("span", { class: "w", title: r.score.toFixed(3) }, h("i", { style: { width: `${(r.score / max) * 100}%` } })),
      h("a", { class: "p", href: href("files", { path: r.path }) }, r.path), " ",
      h("a", { class: "small", href: href("graph", { path: r.path }) }, "graph"),
      r.via?.length ? h("div", { class: "via" }, r.via.join("  ")) : null))),
  ];
}

function renderMap(records) {
  const rows = records.filter((r) => r.type === "map").map((r) => r.data);
  if (!rows.length) return [empty("Nothing to map.")];
  return [
    h("p", { class: "prose small" }, "An outline of the files most related to the matches, within the token budget: what an agent would be shown as a repository map."),
    rows.map((r) => h("div", { class: "result" },
      h("div", { class: "result-head" },
        h("span", { class: "path" }, h("a", { href: href("files", { path: r.path }) }, pathLabel(r.path))),
        h("span", { class: "count" }, r.score !== undefined ? r.score.toFixed(1) : "")),
      h("div", { class: "lines" }, (r.defs || []).map((d) =>
        h("a", { class: "line context", href: href("files", { path: r.path, line: d.line }) },
          h("span", { class: "ln" }, d.line), h("span", { class: "tx" }, d.signature)))))),
  ];
}

function renderCallers(records) {
  const files = [];
  for (const r of records) {
    if (r.type === "begin") files.push({ path: r.data.path.text, scopes: [] });
    else if (r.type === "scope" && files.length) files[files.length - 1].scopes.push(r.data);
  }
  if (!files.length) return [empty("No callers found.")];
  return [
    h("p", { class: "prose small" }, "The definitions whose bodies use the name, found by name rather than by type: a method with the same name on another class counts too."),
    files.map((f) => h("div", { class: "result" },
      h("div", { class: "result-head" }, h("span", { class: "path" }, h("a", { href: href("files", { path: f.path }) }, pathLabel(f.path)))),
      h("div", { class: "lines" }, f.scopes.map((s) =>
        h("a", { class: "line", href: href("files", { path: f.path, line: s.line_number }) },
          h("span", { class: "ln" }, s.line_number),
          h("span", { class: "tx" }, (s.chain || []).slice(0, -1).map((c) => `${c.name} › `).join(""), s.lines?.text ?? "")))))),
  ];
}

function renderCallees(records) {
  const rows = records.filter((r) => r.type === "callee").map((r) => r.data);
  if (!rows.length) return [empty("No callees found: the pattern must name a definition.")];
  const groups = new Map();
  for (const r of rows) {
    const k = `${r.from.path}:${r.from.line}`;
    if (!groups.has(k)) groups.set(k, { from: r.from, rows: [] });
    groups.get(k).rows.push(r);
  }
  return [
    h("p", { class: "prose small" }, "Names used in the body of each definition, resolved to where they are defined, by name."),
    [...groups.values()].map((g) => h("div", { class: "result" },
      h("div", { class: "result-head" },
        h("span", { class: "path" }, h("a", { href: href("files", { path: g.from.path, line: g.from.line }) }, pathLabel(g.from.path), `:${g.from.line}`)),
        h("span", { class: "count mono" }, g.from.name)),
      h("div", { class: "lines" }, g.rows.map((r) =>
        h("a", { class: "line", href: href("files", { path: r.path, line: r.line }) },
          h("span", { class: "ln" }, r.kind.slice(0, 3)),
          h("span", { class: "tx" }, r.signature, "  ", h("span", { class: "faint" }, `${r.path}:${r.line}`))))))),
  ];
}

// ── Plan ─────────────────────────────────────────────────

function funnelRow(label, value, total, cls = "") {
  // Log scale: a funnel from 60,000 files to 3 would otherwise show one bar.
  const w = value <= 0 ? 0 : Math.max(1.5, (Math.log10(value + 1) / Math.log10(total + 1)) * 100);
  return [h("span", { class: "k" }, label), h("span", { class: "track" }, h("i", { class: cls, style: { width: `${w}%` } })), h("span", { class: "v" }, n(value))];
}

function keyPill(k) {
  const title = [
    `${k.kind} ${k.key}`,
    `${n(k.postings)} files`,
    k.dropped ? "left out of the index as too common" : null,
    k.fallback ? "fallback trigram: used only when grams cannot cover the run" : null,
    k.used === false ? "not read: the planner had enough without it" : null,
  ].filter(Boolean).join("\n");
  return h("span", { class: `pill ${k.kind === "gram" ? "gram" : ""} ${k.used === false ? "unused" : ""}`, title },
    k.text ?? `#${k.key}`, " ", h("span", { class: "faint" }, k.dropped ? "pruned" : compact(k.postings)));
}

function planNode(node) {
  if (node.op === "scan") return h("div", { class: "note" }, "No literal to look up: every file is read.");
  if (node.op === "and") return h("div", { class: "keys" }, node.keys.map(keyPill));
  return h("div", { class: "node" },
    h("div", { class: "op" }, node.op === "or" ? "any of" : "all of"),
    node.children.map((c) => h("div", { class: "node" }, planNode(c))));
}

function renderPlan(p) {
  const f = p.funnel;
  const precision = f.verified ? f.matched / f.verified : null;
  return [
    h("div", { class: "funnel" },
      funnelRow("Indexed", f.files, f.files),
      funnelRow("Candidates", f.candidates, f.files),
      funnelRow(f.sample ? "Matched (sample)" : "Matched", f.matched, f.files, "hit")),
    h("p", { class: "small muted" },
      p.match_all ? "No usable literal: a full scan. " : `${pct(f.candidates / Math.max(1, f.files), 2)} of files read; `,
      precision !== null ? `${pct(precision)} of those match. ` : "",
      `${n(f.lines)} matching lines, ${compact(f.bytes_read)}B read.`),
    h("dl", { class: "dl small" },
      h("dt", null, "Plan"), h("dd", null, `${p.timing.plan_us} µs`),
      h("dt", null, "Index"), h("dd", null, `${p.timing.index_us} µs`, h("span", { class: "faint" }, ` · ${p.work.lists_loaded} lists, ${p.work.lists_probed} probes, ${n(p.work.postings_copied)} postings`)),
      h("dt", null, "Verify"), h("dd", null, ms(p.timing.verify_ms), h("span", { class: "faint" }, " in this process")),
      h("dt", null, "Keys"), h("dd", null, p.sparse_index ? "trigrams + sparse grams" : "trigrams only", p.ignore_case ? ", case-folded" : "")),
    h("h3", null, "Execution order"),
    p.trace.length
      ? h("table", { class: "trace" }, h("tbody", null, p.trace.map((t, i) => h("tr", null,
          h("td", { class: "faint" }, i + 1),
          h("td", null, h("span", { class: `pill ${t.kind === "gram" ? "gram" : ""}` }, t.kind)),
          h("td", { class: "key" }, t.text ?? `#${t.key}`),
          h("td", { class: "num" }, t.op === "load" ? `load ${compact(t.postings)}` : `probe ${n(t.candidates)} → ${n(t.hits)}`)))))
      : h("p", { class: "faint small" }, "No posting list was read."),
    h("h3", null, "Plan"),
    h("div", { class: "tree" }, planNode(p.plan)),
    h("p", { class: "faint small" }, "Grams are case-folded keys of 4-8 bytes; faded keys were not read. Counts are files per key. The trace is the planner's real order, rarest first, against the index on disk."),
  ];
}

// ── Running ──────────────────────────────────────────────

async function run() {
  syncUrl();
  const key = JSON.stringify(st);
  if (key === lastKey) return;
  lastKey = key;
  if (!st.q) {
    clear(els.results, empty("Type a pattern. ", h("kbd", null, "/"), " focuses the search box from anywhere."));
    clear(els.status);
    clear(els.plan, h("p", { class: "faint small" }, "The query plan appears here."));
    lastPlanKey = null;
    return;
  }
  clear(els.status, h("span", { class: "loading" }, "Searching"));
  loadPlan();
  try {
    let data, body;
    if (st.lens === "matches") {
      data = await api("search", {
        ...params(), rank: st.rank === "1", scope: st.scope === "1", kind: st.kind, in: st.in, glob: st.glob, context: st.context,
      }, { slot: "search" });
      body = renderMatches(data);
      const sm = data.summary || {};
      clear(els.status,
        h("span", null, h("b", null, n(sm.matched_lines ?? 0)), " lines in ", h("b", null, n(data.files.length)), " files"),
        data.truncated ? h("span", { class: "accent" }, "stopped at 20,000 records") : null,
        h("span", null, ms(data.elapsed_ms)),
        cmd(data.command));
    } else {
      const extra = st.lens === "map" ? { tokens: 1500 } : {};
      data = await api("feature", { mode: st.lens, ...params(), ...extra }, { slot: "search" });
      body = { near: renderNear, map: renderMap, callers: renderCallers, callees: renderCallees }[st.lens](data.records);
      clear(els.status, h("span", null, ms(data.elapsed_ms)), cmd(data.command));
    }
    clear(els.results, data.notes ? h("div", { class: "note" }, data.notes) : null, body);
  } catch (e) {
    if (aborted(e)) return;
    clear(els.status);
    clear(els.results, errorBox(e));
  }
}

function cmd(command) {
  return h("span", { class: "cmd" }, h("code", { title: command }, command), h("button", { class: "btn link small", type: "button", onclick: () => copy(command) }, "copy"));
}

async function loadPlan() {
  // Ranking, scope, filters and lenses do not change the plan; verifying it
  // can read thousands of files, so only a new pattern or option re-plans.
  const key = JSON.stringify(params());
  if (key === lastPlanKey) return;
  lastPlanKey = key;
  clear(els.plan, h("div", { class: "loading small" }, "Planning"));
  try {
    const p = await api("plan", params(), { slot: "plan" });
    clear(els.plan, renderPlan(p));
  } catch (e) {
    if (aborted(e)) return;
    lastPlanKey = null;
    clear(els.plan, errorBox(e));
  }
}

const runSoon = debounce(run, 300);

function set(k, v) {
  st[k] = v;
  run();
}

function mount(root) {
  const input = h("input", {
    class: "field query", placeholder: "Pattern (regex)", "aria-label": "Search pattern", "data-primary": "", spellcheck: "false", autocomplete: "off",
    oninput: () => { st.q = input.value; runSoon(); },
    onkeydown: (e) => { if (e.key === "Enter") { st.q = input.value; lastKey = lastPlanKey = null; run(); } },
  });
  const placeholder = () => { input.placeholder = st.fixed ? "Pattern (fixed string)" : "Pattern (regex)"; };
  const mode = segmented([
    { value: "", label: ".*", title: "Regular expression", mono: true },
    { value: "1", label: "abc", title: "Fixed string", mono: true },
  ], st.fixed, (v) => { set("fixed", v); placeholder(); });
  const cs = segmented([
    { value: "smart", label: "Smart", title: "Ignore case unless the pattern has uppercase (-S)" },
    { value: "sensitive", label: "Aa", title: "Case-sensitive (-s)" },
    { value: "insensitive", label: "aa", title: "Ignore case (-i)" },
  ], st.case, (v) => set("case", v));
  const word = toggle("\\b word", st.word === "1", (v) => set("word", v ? "1" : ""), "Whole words (-w)");
  const rank = toggle("Rank", st.rank === "1", (v) => set("rank", v ? "1" : ""), "Best files first, with the features behind each score (--rank)");
  const scope = toggle("Scope", st.scope === "1", (v) => set("scope", v ? "1" : ""), "Show the enclosing definitions of each match (--scope)");
  const kind = h("select", { class: "field", "aria-label": "Definitions or references", onchange: () => set("kind", kind.value) },
    h("option", { value: "" }, "defs + refs"), h("option", { value: "def" }, "definitions"), h("option", { value: "ref" }, "references"));
  const zone = h("select", { class: "field", "aria-label": "Where in the code", onchange: () => set("in", zone.value) },
    h("option", { value: "" }, "anywhere"), h("option", { value: "code" }, "code only"), h("option", { value: "comments" }, "comments"), h("option", { value: "strings" }, "strings"));
  const ctx = h("select", { class: "field", "aria-label": "Context lines", onchange: () => set("context", ctx.value) },
    ["0", "1", "2", "3", "5"].map((v) => h("option", { value: v }, v === "0" ? "no context" : `±${v} lines`)));
  const glob = h("input", { class: "field narrow mono", placeholder: "glob, e.g. *.py", "aria-label": "Path glob", spellcheck: "false",
    onchange: () => set("glob", glob.value.trim()) });

  const lensBar = h("div", { class: "lenses", role: "tablist" });
  const drawLenses = () => clear(lensBar, LENSES.map((l) => h("button", {
    type: "button", role: "tab", "aria-pressed": String(st.lens === l.id),
    onclick: () => { st.lens = l.id; drawLenses(); run(); },
  }, l.label)));
  drawLenses();

  els = {
    input, mode, cs, word, rank, scope, kind, zone, ctx, glob, drawLenses, placeholder,
    status: h("div", { class: "statusline", "aria-live": "polite" }),
    results: h("div"),
    plan: h("div"),
  };
  clear(root, h("div", { class: "split rail-right" },
    h("div", { class: "pane" },
      h("div", { class: "querybar" }, input, mode, cs, h("div", { class: "seg" }, word)),
      h("div", { class: "querybar" }, h("div", { class: "seg" }, rank, scope), kind, zone, ctx, glob,
        h("a", { class: "small", href: "#", onclick: (e) => { e.preventDefault(); location.hash = href("history", { q: st.q, fixed: st.fixed, case: st.case }); } }, "History of this pattern")),
      lensBar, els.status, els.results),
    h("aside", { class: "rail", "aria-label": "Query plan" }, h("h2", null, "Query plan"), els.plan)));
}

function route(p) {
  // A bare tab link returns to the search as it was left.
  if (!Object.keys(p).length && st.q) { syncUrl(); return; }
  const next = { ...DEFAULTS, ...p };
  const changed = JSON.stringify(next) !== JSON.stringify(st);
  st = next;
  els.input.value = st.q;
  els.mode.set(st.fixed);
  els.placeholder();
  els.cs.set(st.case);
  els.word.set(st.word === "1");
  els.rank.set(st.rank === "1");
  els.scope.set(st.scope === "1");
  els.kind.value = st.kind;
  els.zone.value = st.in;
  els.ctx.value = st.context;
  els.glob.value = st.glob;
  els.drawLenses();
  if (changed || lastKey === null) run();
  if (!st.q) els.input.focus();
}

export default { id: "search", title: "Search", mount, route };
