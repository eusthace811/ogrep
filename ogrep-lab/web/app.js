// The shell: header facts, tabs, routing between views, the go-to palette,
// keyboard shortcuts and the theme switch. Each view keeps its own container
// alive while hidden, so switching tabs never loses a search or a scroll
// position.

import { h, clear, api, parseHash, href, go, n, ago, state, aborted } from "./lib.js";
import indexView from "./views/index.js";
import searchView from "./views/search.js";
import filesView from "./views/files.js";
import graphView from "./views/graph.js";
import historyView from "./views/history.js";
import opsView from "./views/ops.js";
import evalsView from "./views/evals.js";

const VIEWS = [indexView, searchView, filesView, graphView, historyView, opsView, evalsView];
const main = document.getElementById("main");
const tabs = document.getElementById("tabs");
const mounted = new Map();

function renderTabs(active) {
  clear(tabs, VIEWS.map((v, i) =>
    h("a", { href: href(v.id), "aria-current": v.id === active ? "page" : null, title: `${v.title} (${i + 1})` }, v.title)));
}

function route() {
  const { view, params } = parseHash();
  const target = VIEWS.find((v) => v.id === view) || VIEWS[0];
  renderTabs(target.id);
  for (const [id, m] of mounted) m.el.hidden = id !== target.id;
  let m = mounted.get(target.id);
  if (!m) {
    const el = h("section", { class: "view", "aria-label": target.title });
    main.appendChild(el);
    m = { el, view: target };
    mounted.set(target.id, m);
    target.mount(el);
  }
  document.title = `${target.title} · ogrep lab`;
  target.route(params);
}

async function loadFacts() {
  const facts = document.getElementById("facts");
  try {
    const o = await api("overview");
    state.overview = o;
    const repo = String(o.root).split(/[\\/]/).filter(Boolean).pop();
    const stale = o.graph && !o.graph.current;
    clear(facts,
      h("span", { title: o.root }, h("b", null, repo)),
      h("span", null, `${n(o.meta.files)} files`),
      h("span", { title: `updated ${new Date(o.meta.updated_at * 1000).toLocaleString()}` }, `indexed ${ago(o.meta.updated_at)}`),
      o.meta.sparse_grams ? h("span", null, "sparse grams") : h("span", { class: "warn" }, "no sparse grams"),
      o.history ? h("span", null, `history ${n(o.history.commits)} commits`) : null,
      o.graph ? h("span", { class: stale ? "warn" : "", title: stale ? "The graph was built from an older file table; ogrep index --graph refreshes it" : "" }, stale ? "graph stale" : "graph") : null,
      o.ogrep_version ? h("span", { title: o.ogrep }, `ogrep ${o.ogrep_version}`) : h("span", { class: "warn" }, "ogrep not found"),
    );
    document.title = `${repo} · ogrep lab`;
  } catch (e) {
    if (!aborted(e)) clear(facts, h("span", { class: "warn" }, e.message));
  }
}

// ── Go to file / definition ──────────────────────────────

function palette() {
  if (document.querySelector(".palette-back")) return;
  const input = h("input", { placeholder: "Path, or @name for a definition", "aria-label": "Go to", autocomplete: "off", spellcheck: "false" });
  const list = h("ul", { role: "listbox" });
  const hint = h("div", { class: "hint" }, "Enter opens · ↑↓ move · Esc closes");
  const back = h("div", { class: "palette-back", onclick: (e) => { if (e.target === back) close(); } },
    h("div", { class: "palette", role: "dialog", "aria-label": "Go to file" }, input, hint, list));
  let items = [];
  let sel = 0;
  const close = () => { back.remove(); document.removeEventListener("keydown", onKey, true); };
  const pick = (it) => {
    close();
    go("files", { path: it.path, line: it.line });
  };
  const draw = () => {
    clear(list, items.map((it, i) => {
      const slash = it.path.lastIndexOf("/");
      return h("li", { role: "option", "aria-selected": String(i === sel), onclick: () => pick(it) },
        it.symbol
          ? [h("b", null, it.symbol), h("span", { class: "dir" }, `${it.path}:${it.line}`), h("span", { class: "side" }, it.kind)]
          : [h("span", null, h("span", { class: "dir" }, it.path.slice(0, slash + 1)), it.path.slice(slash + 1))]);
    }));
    list.children[sel]?.scrollIntoView({ block: "nearest" });
  };
  const search = async () => {
    const q = input.value.trim();
    try {
      if (q.startsWith("@")) {
        if (q.length < 2) { items = []; draw(); return; }
        const r = await api("symbols", { q: q.slice(1), limit: 40 }, { slot: "palette" });
        items = r.symbols.map((s) => ({ symbol: s.name, path: s.path, line: s.line, kind: s.kind }));
      } else {
        const r = await api("find", { q, limit: 40 }, { slot: "palette" });
        items = r.paths.map((p) => ({ path: p }));
      }
      sel = 0;
      draw();
    } catch (e) {
      if (!aborted(e)) clear(list, h("li", null, e.message));
    }
  };
  const onKey = (e) => {
    if (e.key === "Escape") { e.preventDefault(); close(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); sel = Math.min(items.length - 1, sel + 1); draw(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); sel = Math.max(0, sel - 1); draw(); }
    else if (e.key === "Enter" && items[sel]) { e.preventDefault(); pick(items[sel]); }
  };
  input.addEventListener("input", search);
  document.addEventListener("keydown", onKey, true);
  document.body.appendChild(back);
  input.focus();
}

// ── Keyboard ─────────────────────────────────────────────

function typing(el) {
  return el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); palette(); return; }
  if (typing(document.activeElement) || e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.key === "/") {
    const input = main.querySelector(".view:not([hidden]) [data-primary]");
    if (input) { e.preventDefault(); input.focus(); input.select?.(); }
  } else if (/^[1-7]$/.test(e.key)) {
    go(VIEWS[Number(e.key) - 1].id);
  }
});

// ── Theme ────────────────────────────────────────────────

function applyTheme(t) {
  if (t) document.documentElement.dataset.theme = t;
  else delete document.documentElement.dataset.theme;
}
let theme = null;
try { theme = localStorage.getItem("ogrep-lab-theme"); } catch { /* storage may be unavailable */ }
applyTheme(theme);
document.getElementById("theme").addEventListener("click", () => {
  const dark = theme ? theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  theme = dark ? "light" : "dark";
  applyTheme(theme);
  try { localStorage.setItem("ogrep-lab-theme", theme); } catch { /* ignore */ }
  window.dispatchEvent(new Event("themechange"));
});

document.getElementById("goto").addEventListener("click", palette);
window.addEventListener("hashchange", route);
loadFacts();
route();
