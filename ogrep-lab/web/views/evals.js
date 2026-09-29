// Evals: run the suites, follow a run, read saved runs, chart them over time
// and compare two.

import { h, clear, api, aborted, n, ms, pct, metric, datetime, ago, short, href, go, loading, errorBox, empty, table, segmented, toast } from "../lib.js";
import { lines, legend } from "../charts.js";

const PARAMS = {
  localize: [
    { key: "tasks", label: "Tasks", type: "number", value: 200, min: 10, max: 2000 },
    { key: "all_parents", label: "Follow merged branches", type: "check", value: false, title: "For repositories whose first-parent history is only merge commits" },
    { key: "variants", label: "Orders", type: "multi", options: ["plain", "path", "count", "rank"], value: ["plain", "path", "count", "rank"] },
  ],
  latency: [
    { key: "queries", label: "Queries", type: "number", value: 40, min: 5, max: 1000 },
    { key: "rounds", label: "Rounds", type: "number", value: 5, min: 1, max: 50 },
    { key: "variants", label: "Variants", type: "multi", options: ["plain", "files", "ignore-case", "rank", "scope", "near"], value: ["plain", "rank"] },
  ],
  history: [
    { key: "patterns", label: "Patterns", type: "number", value: 20, min: 5, max: 500 },
  ],
};

/// The one figure per variant that a runs table shows.
const HEADLINE = { localize: "mrr", latency: "p50_ms", history: "agreement" };

let els = {};
let data = null; // { suites, runs, job }
let filter = "";
let poll = null;
let params = {};
let selected = [];

/// Variants in the order the run was asked for (a JSON object's key order is
/// not kept on the server).
function variantsOf(run) {
  const asked = (run.params?.variants || []).filter((v) => run.summary && v in run.summary);
  return asked.length ? asked : Object.keys(run.summary || {});
}

function took(s) {
  if (s < 10) return `${s.toFixed(1)} s`;
  if (s < 120) return `${Math.round(s)} s`;
  return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`;
}

function suiteOf(id) {
  return data?.suites.find((s) => s.id === id);
}

// ── Suites ───────────────────────────────────────────────

function paramControls(suite) {
  const values = (params[suite.id] ||= Object.fromEntries(PARAMS[suite.id].map((p) => [p.key, Array.isArray(p.value) ? [...p.value] : p.value])));
  return PARAMS[suite.id].map((p) => {
    if (p.type === "number") {
      const el = h("input", { class: "field", type: "number", min: p.min, max: p.max, value: values[p.key],
        onchange: () => { values[p.key] = Math.max(p.min, Math.min(p.max, Number(el.value) || p.value)); el.value = values[p.key]; } });
      return h("label", null, p.label, el);
    }
    if (p.type === "check") {
      const el = h("input", { type: "checkbox", checked: values[p.key], onchange: () => { values[p.key] = el.checked; } });
      return h("label", { class: "check", title: p.title }, el, p.label);
    }
    return h("span", { class: "params" }, h("span", { class: "faint" }, p.label),
      p.options.map((o) => {
        const el = h("input", { type: "checkbox", checked: values[p.key].includes(o), onchange: () => {
          values[p.key] = el.checked ? [...values[p.key], o] : values[p.key].filter((x) => x !== o);
        } });
        return h("label", { class: "check" }, el, o);
      }));
  });
}

function drawSuites() {
  const running = data.job?.running;
  clear(els.suites, h("div", { class: "suites" }, data.suites.map((s) => {
    const binary = h("input", { class: "field", placeholder: "default: the lab's ogrep", spellcheck: "false",
      value: params[s.id]?.ogrep || "", oninput: () => { (params[s.id] ||= {}).ogrep = binary.value; } });
    return h("div", { class: "suite" },
      h("h3", { style: { margin: 0 } }, s.title),
      h("div", { class: "about" }, s.about),
      s.ready ? h("div", { class: "blocked" }, s.ready) : [
        h("div", { class: "params" }, paramControls(s)),
        h("details", { class: "advanced" }, h("summary", null, "Another ogrep binary"),
          h("p", { class: "faint small" }, "To compare builds: run once with each, then compare the two runs."), binary),
      ],
      h("div", null, h("button", {
        class: "btn primary", type: "button", disabled: !!(running || s.ready),
        onclick: () => start(s.id),
      }, running ? "A run is in progress" : "Run")));
  })));
}

async function start(suiteId) {
  try {
    const job = await api("evals/start", {}, { body: { suite: suiteId, params: params[suiteId] || {} } });
    data.job = job;
    drawSuites();
    drawJob();
    watch();
  } catch (e) {
    toast(e.message);
  }
}

// ── Job ──────────────────────────────────────────────────

function drawJob() {
  const j = data.job;
  if (!j) { clear(els.job); return; }
  const frac = j.total ? j.done / j.total : 0;
  const finished = !j.running;
  if (finished && !j.error && !j.run) { clear(els.job); return; }
  clear(els.job, h("div", { class: "job", role: "status", "aria-live": "polite" },
    h("div", { class: "top" },
      h("b", null, suiteOf(j.suite)?.title || j.suite),
      h("span", { class: "muted" }, j.running ? j.phase : j.error ? "failed" : "finished"),
      j.total ? h("span", { class: "faint" }, `${n(j.done)} / ${n(j.total)}`) : null,
      h("span", { class: "faint" }, took(j.elapsed_s)),
      h("span", { style: { marginLeft: "auto" } },
        j.running ? h("button", { class: "btn", type: "button", onclick: () => api("evals/cancel", {}, { body: {} }) }, "Cancel") : null,
        !j.running && j.run ? h("a", { href: href("evals", { run: j.run }) }, "Open the run") : null)),
    j.running ? h("div", { class: "progress" }, h("i", { style: { width: `${(frac * 100).toFixed(1)}%` } })) : null,
    j.error ? h("div", { class: "error" }, j.error) : null,
    j.log?.length ? h("pre", null, j.log.join("\n")) : null));
}

function watch() {
  clearInterval(poll);
  poll = setInterval(async () => {
    try {
      const job = await api("job");
      const wasRunning = data.job?.running;
      data.job = job;
      drawJob();
      if (!job?.running) {
        clearInterval(poll);
        poll = null;
        if (wasRunning) {
          await refresh();
          if (job.run) go("evals", { run: job.run });
        }
      }
    } catch { /* keep polling */ }
  }, 700);
}

// ── Runs ─────────────────────────────────────────────────

function headline(run) {
  const key = HEADLINE[run.suite];
  const meta = suiteOf(run.suite)?.metrics.find((m) => m.key === key);
  return variantsOf(run).map((variant) =>
    h("span", { style: { marginRight: "12px", whiteSpace: "nowrap" } }, h("span", { class: "faint" }, `${variant} `), metric(run.summary[variant][key], meta?.unit)));
}

function drawRuns() {
  const runs = data.runs.filter((r) => !filter || r.suite === filter);
  if (!data.runs.length) {
    clear(els.runs, empty("No runs yet. Runs are saved under the index, in lab/runs/, one JSON file each."));
    clear(els.trends);
    return;
  }
  const cmp = selected.length === 2
    ? h("a", { class: "btn primary", href: href("evals", { a: selected[0], b: selected[1] }) }, "Compare the two selected")
    : h("span", { class: "faint small" }, "Tick two runs of the same suite to compare them.");
  clear(els.runs,
    h("div", { class: "querybar" },
      segmented([{ value: "", label: "All" }, ...data.suites.map((s) => ({ value: s.id, label: s.title }))], filter, (v) => { filter = v; drawRuns(); }),
      cmp),
    table([
      { key: "pick", label: "", sortable: false, render: (r) => {
        const box = h("input", { type: "checkbox", "aria-label": "Select for comparison", checked: selected.includes(r.id),
          onclick: (e) => e.stopPropagation(),
          onchange: () => {
            selected = box.checked ? [...selected.filter((x) => x !== r.id), r.id].slice(-2) : selected.filter((x) => x !== r.id);
            drawRuns();
          } });
        return box;
      } },
      { key: "started_at", label: "When", num: false, render: (r) => h("span", { title: r.id }, datetime(r.started_at)) },
      { key: "title", label: "Suite" },
      { key: "ogrep", label: "ogrep", sort: (r) => r.env?.ogrep_version, render: (r) => h("span", { class: "mono small", title: r.env?.ogrep }, r.env?.ogrep_version || "?") },
      { key: "head", label: "Repository at", sort: (r) => r.env?.head, render: (r) => h("span", { class: "mono small" }, short(r.env?.head)) },
      { key: "result", label: "Result", sortable: false, render: headline },
      { key: "duration_s", label: "Took", num: true, render: (r) => took(r.duration_s) },
      { key: "del", label: "", sortable: false, render: (r) => {
        // Two clicks, no modal: the first arms the button for a few seconds.
        const b = h("button", { class: "btn link small", type: "button", title: "Delete this run", onclick: async (e) => {
          e.stopPropagation();
          if (b.dataset.armed !== "1") {
            b.dataset.armed = "1";
            b.textContent = "confirm delete";
            setTimeout(() => { b.dataset.armed = ""; b.textContent = "delete"; }, 3000);
            return;
          }
          await api("evals/delete", {}, { body: { id: r.id } }).catch((err) => toast(err.message));
          selected = selected.filter((x) => x !== r.id);
          refresh();
        } }, "delete");
        return b;
      } },
    ], runs, { onclick: (r) => go("evals", { run: r.id }), initial: { key: "started_at", dir: -1 } }));
  drawTrends(runs);
}

function drawTrends(runs) {
  if (!filter) { clear(els.trends, runs.length > 1 ? h("p", { class: "faint small" }, "Pick a suite above to chart its runs over time.") : null); return; }
  const suite = suiteOf(filter);
  const ordered = [...runs].sort((a, b) => a.started_at - b.started_at);
  if (ordered.length < 2) { clear(els.trends, h("p", { class: "faint small" }, "Two or more runs of this suite make a trend.")); return; }
  const variants = [...new Set(ordered.flatMap((r) => Object.keys(r.summary || {})))];
  clear(els.trends,
    h("h2", null, `${suite.title} over time`),
    legend(variants),
    h("div", { class: "smalls" }, suite.metrics.map((m) => {
      const series = variants.map((v) => ({
        name: v,
        points: ordered.filter((r) => r.summary?.[v]?.[m.key] !== undefined).map((r) => ({
          x: r.started_at, y: r.summary[v][m.key], label: `${datetime(r.started_at)} · ogrep ${r.env?.ogrep_version || "?"} · ${short(r.env?.head)}`,
        })),
      })).filter((se) => se.points.length);
      return h("div", { class: "small-chart" }, h("div", { class: "t" }, m.label),
        lines(series, { format: (v) => metric(v, m.unit), higherIsBetter: m.higher_is_better }));
    })));
}

// ── One run ──────────────────────────────────────────────

function summaryTable(suite, run) {
  const summary = run.summary;
  const variants = variantsOf(run);
  return h("table", { class: "grid" },
    h("thead", null, h("tr", null, h("th", null, ""), suite.metrics.map((m) => h("th", { class: "num" }, m.label)))),
    h("tbody", null, variants.map((v) => h("tr", null, h("td", null, h("b", null, v)),
      suite.metrics.map((m) => {
        const vals = variants.map((x) => summary[x][m.key]).filter((x) => x !== undefined);
        const best = m.higher_is_better ? Math.max(...vals) : Math.min(...vals);
        const val = summary[v][m.key];
        // Mark the best only where the variants differ.
        const differ = vals.some((x) => x !== best);
        return h("td", { class: `num ${differ && val === best ? "good" : ""}` }, metric(val, m.unit));
      })))));
}

function posCell(p) {
  if (p === null || p === undefined) return h("span", { class: "pos miss" }, "–");
  return h("span", { class: `pos ${p === 1 ? "p1" : p > 10 ? "far" : ""}` }, p);
}

function itemsTable(run) {
  const variants = variantsOf(run);
  if (run.suite === "localize") {
    let show = "all";
    const box = h("div");
    const has = (v) => variants.includes(v);
    const draw = () => {
      let rows = run.items;
      const pos = (r, v) => r.positions[v] ?? Infinity;
      if (show === "better") rows = rows.filter((r) => pos(r, "rank") < pos(r, "plain"));
      if (show === "worse") rows = rows.filter((r) => pos(r, "rank") > pos(r, "plain"));
      if (show === "missed") rows = rows.filter((r) => variants.some((v) => r.positions[v] === null));
      clear(box, h("p", { class: "faint small" }, `${n(rows.length)} tasks`), table([
        { key: "query", label: "Query", render: (r) => h("span", { class: "mono" }, r.query) },
        { key: "subject", label: "Commit", render: (r) => h("span", { title: `${r.commit}\n${r.subject}` }, h("span", { class: "mono faint small" }, short(r.commit)), " ", r.subject.length > 80 ? r.subject.slice(0, 78) + "…" : r.subject) },
        { key: "relevant", label: "Changed files", sort: (r) => r.relevant[0], render: (r) => h("div", { class: "small" }, r.relevant.map((p) => h("div", null, h("a", { class: "mono", href: href("files", { path: p }) }, p)))) },
        ...variants.map((v) => ({ key: `p_${v}`, label: v, num: true, sort: (r) => r.positions[v] ?? 1e9, render: (r) => posCell(r.positions[v]) })),
        { key: "open", label: "", sortable: false, render: (r) => h("a", { class: "small", href: href("search", { q: r.query, fixed: "1", rank: "1" }) }, "search") },
      ], rows));
    };
    const tabs = has("rank") && has("plain")
      ? segmented([{ value: "all", label: "All" }, { value: "better", label: "Rank beats plain" }, { value: "worse", label: "Rank loses to plain" }, { value: "missed", label: "Missed" }], show, (v) => { show = v; draw(); })
      : null;
    draw();
    return [tabs, box];
  }
  if (run.suite === "latency") {
    return table([
      { key: "query", label: "Query", render: (r) => h("span", { class: "mono" }, r.query) },
      ...variants.map((v) => ({ key: `ms_${v}`, label: v, num: true, sort: (r) => r.ms[v], render: (r) => ms(r.ms[v]) })),
    ], run.items, { initial: { key: `ms_${variants[0]}`, dir: -1 } });
  }
  return table([
    { key: "query", label: "Pattern", render: (r) => h("span", { class: "mono" }, r.query) },
    { key: "agree", label: "Agrees", render: (r) => (r.agree ? h("span", { class: "good" }, "yes") : h("span", { class: "bad" }, "no")) },
    { key: "commits", label: "git", num: true },
    { key: "ogrep_commits", label: "ogrep", num: true },
    { key: "missing", label: "Missing / extra", sortable: false, render: (r) => h("span", { class: "mono small" }, [...r.missing.map((x) => `-${short(x)}`), ...r.extra.map((x) => `+${short(x)}`)].join(" ") || "–") },
    { key: "ogrep_ms", label: "ogrep", num: true, render: (r) => ms(r.ogrep_ms) },
    { key: "git_ms", label: "git log -G", num: true, render: (r) => ms(r.git_ms) },
  ], run.items, { initial: { key: "agree", dir: 1 } });
}

function envList(run) {
  const e = run.env || {};
  return h("dl", { class: "dl small" },
    h("dt", null, "ogrep"), h("dd", { class: "mono" }, `${e.ogrep_version || "?"}  ${e.ogrep || ""}`),
    h("dt", null, "Repository at"), h("dd", { class: "mono" }, e.head || "not a git checkout"),
    h("dt", null, "Index"), h("dd", null, `${n(e.files)} files, updated ${datetime(e.index_updated_at)}${e.sparse_grams ? ", sparse grams" : ", no sparse grams"}`),
    h("dt", null, "Parameters"), h("dd", { class: "mono" }, Object.entries(run.params || {}).filter(([k]) => k !== "ogrep").map(([k, v]) => `${k}=${Array.isArray(v) ? v.join(",") : v}`).join("  ")),
    h("dt", null, "Ran"), h("dd", null, `${datetime(run.started_at)} (${ago(run.started_at)}), took ${took(run.duration_s)}`));
}

async function showRun(id) {
  clear(els.detail, loading());
  try {
    const run = await api("evals/run", { id });
    const suite = suiteOf(run.suite);
    clear(els.detail,
      h("p", null, h("a", { href: href("evals") }, "All runs")),
      h("h1", null, run.title),
      h("p", { class: "faint small mono" }, run.id),
      envList(run),
      h("h2", null, "Summary"),
      summaryTable(suite, run),
      h("h2", null, run.suite === "localize" ? "Tasks" : run.suite === "latency" ? "Per query (median of rounds)" : "Patterns"),
      itemsTable(run));
  } catch (e) {
    if (!aborted(e)) clear(els.detail, h("p", null, h("a", { href: href("evals") }, "All runs")), errorBox(e));
  }
}

// ── Two runs ─────────────────────────────────────────────

function deltaCell(a, b, m) {
  if (a === undefined || b === undefined) return h("td", { class: "num faint" }, "–");
  const d = b - a;
  const better = m.higher_is_better ? d > 0 : d < 0;
  const rel = a ? d / Math.abs(a) : 0;
  // Timings move a few percent between identical runs; do not colour noise.
  const tiny = Math.abs(rel) < (m.unit === "ms" ? 0.03 : 0.005);
  const sign = d > 0 ? "+" : d < 0 ? "\u2212" : "\u00b1";
  const text = m.unit === "pct" ? `${sign}${Math.abs(d * 100).toFixed(1)} pt` : `${sign}${metric(Math.abs(d), m.unit)}`;
  return h("td", { class: `num ${tiny ? "faint" : better ? "delta-up" : "delta-down"}` }, text, m.unit !== "pct" && a ? h("span", { class: "faint" }, ` (${rel >= 0 ? "+" : ""}${(rel * 100).toFixed(1)}%)`) : null);
}

async function showCompare(a, b) {
  clear(els.detail, loading());
  try {
    const [ra, rb] = await Promise.all([api("evals/run", { id: a }), api("evals/run", { id: b })]);
    const [A, B] = ra.started_at <= rb.started_at ? [ra, rb] : [rb, ra];
    if (A.suite !== B.suite) throw new Error("These runs are of different suites.");
    const suite = suiteOf(A.suite);
    const variants = variantsOf(A).filter((v) => v in B.summary);
    const itemDiff = [];
    if (A.suite === "localize") {
      const key = (r) => `${r.commit}:${r.query}`;
      const byKey = new Map(B.items.map((r) => [key(r), r]));
      for (const r of A.items) {
        const o = byKey.get(key(r));
        if (!o) continue;
        for (const v of variants) if (r.positions[v] !== o.positions[v]) itemDiff.push({ query: r.query, subject: r.subject, variant: v, a: r.positions[v], b: o.positions[v] });
      }
    } else if (A.suite === "latency") {
      const byQ = new Map(B.items.map((r) => [r.query, r]));
      for (const r of A.items) {
        const o = byQ.get(r.query);
        if (o) for (const v of variants) itemDiff.push({ query: r.query, variant: v, a: r.ms[v], b: o.ms[v], ratio: o.ms[v] / r.ms[v] });
      }
    } else {
      const byQ = new Map(B.items.map((r) => [r.query, r]));
      for (const r of A.items) { const o = byQ.get(r.query); if (o && o.agree !== r.agree) itemDiff.push({ query: r.query, a: r.agree, b: o.agree }); }
    }
    const key = (r) => (A.suite === "localize" ? `${r.commit}:${r.query}` : r.query);
    const inB = new Set(B.items.map(key));
    const shared = A.items.filter((r) => inB.has(key(r))).length;
    const sameQueries = shared === A.items.length && shared === B.items.length;
    clear(els.detail,
      h("p", null, h("a", { href: href("evals") }, "All runs")),
      h("h1", null, `${suite.title}: two runs`),
      h("div", { class: "cols" },
        h("div", null, h("h3", null, "A (older)"), envList(A)),
        h("div", null, h("h3", null, "B (newer)"), envList(B))),
      !sameQueries ? h("div", { class: "note" }, `The runs share ${n(shared)} of their ${n(A.items.length)} and ${n(B.items.length)} ${A.suite === "localize" ? "tasks" : "queries"} (different counts, or the repository moved on), so the metrics are not strictly comparable. Per-item differences cover the shared ones.`) : null,
      h("h2", null, "Metrics"),
      h("table", { class: "grid" },
        h("thead", null, h("tr", null, h("th", null, "Variant"), h("th", null, "Metric"), h("th", { class: "num" }, "A"), h("th", { class: "num" }, "B"), h("th", { class: "num" }, "B − A"))),
        h("tbody", null, variants.flatMap((v) => suite.metrics.map((m, i) => h("tr", null,
          h("td", null, i === 0 ? h("b", null, v) : ""),
          h("td", { class: "muted" }, m.label),
          h("td", { class: "num" }, metric(A.summary[v][m.key], m.unit)),
          h("td", { class: "num" }, metric(B.summary[v][m.key], m.unit)),
          deltaCell(A.summary[v][m.key], B.summary[v][m.key], m)))))),
      h("h2", null, A.suite === "localize" ? "Tasks whose position changed" : A.suite === "latency" ? "Per query, B relative to A" : "Patterns whose agreement changed"),
      itemDiff.length ? (A.suite === "localize"
        ? table([
            { key: "query", label: "Query", render: (r) => h("span", { class: "mono" }, r.query) },
            { key: "variant", label: "Order" },
            { key: "a", label: "A", num: true, sort: (r) => r.a ?? 1e9, render: (r) => posCell(r.a) },
            { key: "b", label: "B", num: true, sort: (r) => r.b ?? 1e9, render: (r) => posCell(r.b) },
            { key: "subject", label: "Commit", render: (r) => h("span", { class: "small muted" }, r.subject) },
          ], itemDiff)
        : A.suite === "latency"
          ? table([
              { key: "query", label: "Query", render: (r) => h("span", { class: "mono" }, r.query) },
              { key: "variant", label: "Variant" },
              { key: "a", label: "A", num: true, render: (r) => ms(r.a) },
              { key: "b", label: "B", num: true, render: (r) => ms(r.b) },
              { key: "ratio", label: "B / A", num: true, render: (r) => h("span", { class: r.ratio > 1.1 ? "bad" : r.ratio < 0.9 ? "good" : "" }, `×${r.ratio.toFixed(2)}`) },
            ], itemDiff, { initial: { key: "ratio", dir: -1 } })
          : table([
              { key: "query", label: "Pattern", render: (r) => h("span", { class: "mono" }, r.query) },
              { key: "a", label: "A agrees", render: (r) => String(r.a) },
              { key: "b", label: "B agrees", render: (r) => String(r.b) },
            ], itemDiff))
        : h("p", { class: "faint" }, "None."));
  } catch (e) {
    if (!aborted(e)) clear(els.detail, h("p", null, h("a", { href: href("evals") }, "All runs")), errorBox(e));
  }
}

// ── View ─────────────────────────────────────────────────

async function refresh() {
  data = await api("evals");
  drawSuites();
  drawJob();
  drawRuns();
  if (data.job?.running && !poll) watch();
}

function mount(root) {
  els = { suites: h("div"), job: h("div"), runs: h("div"), trends: h("div", { class: "section" }), detail: h("div") };
  els.home = h("div", null,
    h("h1", null, "Evaluations"),
    h("p", { class: "prose" }, "Each suite runs against this repository and its index, with queries drawn from the repository itself, so the numbers mean something for this codebase. Runs are saved with the ogrep version and commit they ran against."),
    els.suites, els.job,
    h("h2", { class: "section" }, "Runs"), els.runs, els.trends);
  clear(root, h("div", { class: "page" }, els.home, els.detail));
}

async function route(p) {
  try {
    if (!data) await refresh();
  } catch (e) {
    clear(els.detail, errorBox(e));
    return;
  }
  const inDetail = p.run || (p.a && p.b);
  els.home.hidden = !!inDetail;
  els.detail.hidden = !inDetail;
  if (p.run) showRun(p.run);
  else if (p.a && p.b) showCompare(p.a, p.b);
  else refresh().catch(() => {});
}

export default { id: "evals", title: "Evals", mount, route };
