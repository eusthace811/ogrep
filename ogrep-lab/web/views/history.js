// History: when a pattern arrived, when it last left, and every commit that
// added or removed a matching line, over time.

import { h, clear, api, aborted, n, ms, date, short, href, replace, segmented, loading, errorBox, empty, marked, copy, state } from "../lib.js";
import { mirrored } from "../charts.js";

const DEFAULTS = { q: "", fixed: "", case: "smart", since: "", until: "", author: "" };
let st = { ...DEFAULTS };
let els = {};
let lastKey = null;

function args() {
  return { q: st.q, fixed: st.fixed === "1", case: st.case, since: st.since, until: st.until, author: st.author };
}

function sync() {
  const p = {};
  for (const [k, v] of Object.entries(st)) if (v !== DEFAULTS[k]) p[k] = v;
  replace("history", p);
}

function where(x) {
  return h("a", { class: "mono small", href: href("files", { path: x.path }) }, `${x.path}:${x.line_number}`);
}

function introduced(rec) {
  if (!rec) return empty("No commit in the indexed history added or removed a matching line.");
  const commitLink = (c) => h("a", { href: "#", class: "mono", onclick: (e) => { e.preventDefault(); openCommit(c.sha); } }, short(c.sha));
  return h("dl", { class: "intro" },
    h("dt", null, "First added"),
    h("dd", null, rec.first_added
      ? [h("b", null, date(rec.first_added.time)), " in ", commitLink(rec.first_added), " by ", rec.first_added.author.replace(/\s*<.*>$/, ""), h("div", { class: "muted" }, rec.first_added.subject), where(rec.first_added)]
      : "–"),
    h("dt", null, "Last removed"),
    h("dd", null, rec.last_removed
      ? [h("b", null, date(rec.last_removed.time)), " in ", commitLink(rec.last_removed), " by ", rec.last_removed.author.replace(/\s*<.*>$/, ""), h("div", { class: "muted" }, rec.last_removed.subject), where(rec.last_removed)]
      : h("span", { class: "faint" }, "never removed")),
    h("dt", null, "At HEAD"),
    h("dd", null, rec.files_at_head ? `in ${n(rec.files_at_head)} files` : h("span", { class: "accent" }, "gone: no file matches at HEAD")),
    h("dt", null, "Changed by"),
    h("dd", null, `${n(rec.commits)} commits`));
}

function buckets(commits) {
  if (!commits.length) return [];
  const t0 = Math.min(...commits.map((c) => c.time));
  const t1 = Math.max(...commits.map((c) => c.time));
  const start = new Date(t0 * 1000);
  const months = (new Date(t1 * 1000).getUTCFullYear() - start.getUTCFullYear()) * 12 + new Date(t1 * 1000).getUTCMonth() - start.getUTCMonth() + 1;
  const step = months > 240 ? 3 : 1;
  const out = [];
  const y0 = start.getUTCFullYear(), m0 = start.getUTCMonth() - (start.getUTCMonth() % step);
  for (let i = 0; i * step < months + step; i++) {
    const a = Date.UTC(y0, m0 + i * step, 1) / 1000, b = Date.UTC(y0, m0 + (i + 1) * step, 1) / 1000;
    const d = new Date(a * 1000);
    out.push({ t0: a, t1: b, up: 0, down: 0, commits: 0, label: step === 1 ? `${d.toLocaleString("en", { month: "short", timeZone: "UTC" })} ${d.getUTCFullYear()}` : `Q${Math.floor(d.getUTCMonth() / 3) + 1} ${d.getUTCFullYear()}` });
  }
  for (const c of commits) {
    const b = out.find((x) => c.time >= x.t0 && c.time < x.t1);
    if (!b) continue;
    b.commits++;
    for (const m of c.matches) m.change === "added" ? b.up++ : b.down++;
  }
  while (out.length && !out[out.length - 1].commits && out.length > 1) out.pop();
  return out;
}

async function openCommit(sha, into) {
  const target = into || els.diff;
  clear(target, loading("Loading the diff"));
  if (!into) target.scrollIntoView({ block: "start" });
  try {
    const c = await api("commit", { sha });
    const rows = [];
    for (const line of c.diff.split("\n")) {
      let cls = "m";
      if (line.startsWith("diff --git")) { rows.push(h("div", { class: "fh" }, line.replace(/^diff --git a\/(.*) b\/.*$/, "$1"))); continue; }
      if (/^(index |--- |\+\+\+ |new file|deleted file|similarity|rename |old mode|new mode)/.test(line)) continue;
      if (line.startsWith("@@")) cls = "hh";
      else if (line.startsWith("+")) cls = "a";
      else if (line.startsWith("-")) cls = "d";
      rows.push(h("div", { class: cls }, line || " "));
    }
    clear(target,
      into ? null : h("h2", null, "Commit ", h("span", { class: "mono" }, short(c.sha))),
      h("div", { class: "small muted" }, `${c.author.replace(/\s*<.*>$/, "")} · ${date(c.time)} · `, h("a", { href: "#", onclick: (e) => { e.preventDefault(); copy(c.sha); } }, "copy id")),
      h("div", { class: "message" }, c.message),
      h("div", { class: "diff" }, rows),
      c.truncated ? h("div", { class: "note" }, "Diff cut at 600 KB.") : null);
  } catch (e) {
    clear(target, errorBox(e));
  }
}

function commitRow(c) {
  const added = c.matches.filter((m) => m.change === "added").length;
  const removed = c.matches.length - added;
  const body = h("div", { class: "commit-body", hidden: true });
  let filled = false;
  const row = h("div", { class: "commit-row", role: "button", tabindex: "0", "aria-expanded": "false" },
    h("span", { class: "date" }, c.date),
    h("span", { class: "sha" }, c.sha.slice(0, 8)),
    h("span", { class: "subject", title: c.subject }, c.subject),
    h("span", { class: "delta" }, added ? h("span", { class: "good" }, `+${added}`) : null, added && removed ? " " : null, removed ? h("span", { class: "bad" }, `−${removed}`) : null));
  const toggleBody = () => {
    body.hidden = !body.hidden;
    row.setAttribute("aria-expanded", String(!body.hidden));
    if (filled) return;
    filled = true;
    const diff = h("div");
    clear(body,
      h("div", { class: "author" }, c.author.replace(/\s*<.*>$/, "")),
      c.matches.slice(0, 60).map((m) => h("div", { class: `change ${m.change}` },
        h("span", { class: "sign" }, m.change === "added" ? "+" : "−"),
        h("span", null,
          h("div", { class: "loc" }, `${m.path}:${m.line_number}`),
          h("div", { class: "tx" }, marked(m.text, (m.submatches || []).map((s) => [s.start, s.end])))))),
      c.matches.length > 60 ? h("div", { class: "faint small" }, `${c.matches.length - 60} more lines`) : null,
      h("button", { class: "btn link small", type: "button", onclick: () => openCommit(c.sha, diff) }, "Show the whole diff"),
      diff);
  };
  row.addEventListener("click", toggleBody);
  row.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); toggleBody(); } });
  return h("div", { class: "commit", id: `c-${c.sha}` }, row, body);
}

async function run() {
  sync();
  const key = JSON.stringify(st);
  if (key === lastKey) return;
  lastKey = key;
  clear(els.diff);
  if (!st.q) {
    clear(els.out, empty("Type a pattern to see when it entered and left the code."));
    return;
  }
  clear(els.out, loading("Searching history"));
  try {
    const [intro, hist] = await Promise.all([
      api("feature", { mode: "introduced", ...args() }, { slot: "introduced" }),
      api("feature", { mode: "history", ...args(), limit: 5000 }, { slot: "history" }),
    ]);
    const rec = intro.records.find((r) => r.type === "introduced")?.data;
    const commits = hist.records.filter((r) => r.type === "commit").map((r) => r.data);
    const bs = buckets(commits);
    clear(els.out,
      h("div", { class: "statusline" }, h("span", null, h("b", null, n(commits.length)), " commits"), hist.truncated ? h("span", { class: "accent" }, "stopped at 5,000") : null, h("span", null, ms(hist.elapsed_ms)), h("code", null, hist.command)),
      intro.notes || hist.notes ? h("div", { class: "note" }, intro.notes || hist.notes) : null,
      introduced(rec),
      bs.length > 1 ? [
        h("h2", null, "Matching lines per ", bs.length && bs[0].label.startsWith("Q") ? "quarter" : "month"),
        mirrored(bs, { onPick: (b) => {
          const c = commits.find((x) => x.time >= b.t0 && x.time < b.t1);
          if (c) document.getElementById(`c-${c.sha}`)?.scrollIntoView({ block: "center" });
        } }),
        h("div", { class: "legend" }, h("span", null, h("i", { style: { background: "var(--s2)", height: "8px" } }), "added"), h("span", null, h("i", { style: { background: "var(--accent)", height: "8px" } }), "removed")),
      ] : null,
      h("h2", null, "Commits, newest first"),
      commits.length ? commits.map(commitRow) : empty("No commits."));
  } catch (e) {
    if (!aborted(e)) clear(els.out, errorBox(e));
  }
}

function mount(root) {
  const input = h("input", { class: "field query", placeholder: "Pattern (regex)", "aria-label": "Pattern", "data-primary": "", spellcheck: "false",
    onkeydown: (e) => { if (e.key === "Enter") { st.q = input.value; run(); } } });
  const mode = segmented([{ value: "", label: ".*", mono: true, title: "Regular expression" }, { value: "1", label: "abc", mono: true, title: "Fixed string" }], st.fixed, (v) => { st.fixed = v; run(); });
  const cs = segmented([{ value: "smart", label: "Smart" }, { value: "sensitive", label: "Aa" }, { value: "insensitive", label: "aa" }], st.case, (v) => { st.case = v; run(); });
  const field = (key, placeholder, label) => {
    const el = h("input", { class: "field narrow", placeholder, "aria-label": label, onchange: () => { st[key] = el.value.trim(); run(); } });
    return el;
  };
  els = {
    input, mode, cs,
    since: field("since", "since: 2y, 2024-01-01", "Since"),
    until: field("until", "until", "Until"),
    author: field("author", "author", "Author"),
    out: h("div"),
    diff: h("div", { class: "section" }),
  };
  clear(root, h("div", { class: "page" },
    h("div", { class: "querybar" }, input, mode, cs, h("button", { class: "btn primary", type: "button", onclick: () => { st.q = input.value; run(); } }, "Search")),
    h("div", { class: "querybar" }, els.since, els.until, els.author),
    els.out, els.diff));
}

async function route(p) {
  const o = state.overview || (await api("overview").catch(() => null));
  if (o && !o.history) {
    clear(els.out, empty("No history index. ", h("code", null, "ogrep index --history"), " indexes every commit's added and removed lines; it takes about as long as the main index and updates incrementally."));
    return;
  }
  if (!Object.keys(p).length && st.q) { sync(); return; }
  st = { ...DEFAULTS, ...p };
  els.input.value = st.q;
  els.mode.set(st.fixed);
  els.cs.set(st.case);
  els.since.value = st.since;
  els.until.value = st.until;
  els.author.value = st.author;
  run();
  if (!st.q) els.input.focus();
}

export default { id: "history", title: "History", mount, route };
