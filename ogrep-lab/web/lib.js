// Shared helpers: DOM building, the API client, formatting, routing state.
// Everything the page shows is built with text nodes, never innerHTML, so
// repository content cannot inject markup.

export function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v === undefined || v === null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k === "style" && typeof v === "object") {
        // Custom properties (--depth) are only settable through setProperty.
        for (const [p, val] of Object.entries(v)) {
          if (p.startsWith("--")) el.style.setProperty(p, String(val));
          else el.style[p] = val;
        }
      }
      else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
      else if (k === "dataset") Object.assign(el.dataset, v);
      else if (v === true) el.setAttribute(k, "");
      else el.setAttribute(k, v);
    }
  }
  append(el, children);
  return el;
}

function append(el, children) {
  for (const c of children) {
    if (c === undefined || c === null || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else if (c instanceof Node) el.appendChild(c);
    else el.appendChild(document.createTextNode(String(c)));
  }
}

export function clear(el, ...children) {
  el.replaceChildren();
  append(el, children);
  return el;
}

export const svgNS = "http://www.w3.org/2000/svg";
export function s(tag, attrs, ...children) {
  const el = document.createElementNS(svgNS, tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null) continue;
    if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c === undefined || c === null || c === false) continue;
    el.appendChild(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

// ── API ──────────────────────────────────────────────────

export class ApiError extends Error {}

/// GET /api/<path>?params, or POST with a JSON body. Requests for the same
/// slot cancel the previous one, so typing never shows stale results.
const inflight = new Map();
export async function api(path, params = {}, { slot, body } = {}) {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === false || v === "") continue;
    qs.set(k, v === true ? "1" : String(v));
  }
  let signal;
  if (slot) {
    inflight.get(slot)?.abort();
    const ctl = new AbortController();
    inflight.set(slot, ctl);
    signal = ctl.signal;
  }
  const url = `/api/${path}${qs.size ? "?" + qs : ""}`;
  const init = body === undefined
    ? { signal }
    : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal };
  let res;
  try {
    res = await fetch(url, init);
  } catch (e) {
    if (e.name === "AbortError") throw e;
    throw new ApiError("ogrep-lab is not responding; is it still running?");
  }
  if (res.status === 403) throw new ApiError("Access expired: open the URL ogrep-lab printed at start.");
  const data = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
  if (!res.ok || data.error) throw new ApiError(data.error || `HTTP ${res.status}`);
  return data;
}

export const aborted = (e) => e && e.name === "AbortError";

// ── Formatting ───────────────────────────────────────────

const nf = new Intl.NumberFormat("en-US");
export const n = (x) => (x === null || x === undefined ? "–" : nf.format(x));

export function compact(x) {
  if (x === null || x === undefined) return "–";
  if (x < 1000) return String(x);
  if (x < 1e6) return (x / 1e3).toFixed(x < 1e4 ? 1 : 0) + "K";
  if (x < 1e9) return (x / 1e6).toFixed(x < 1e7 ? 2 : 1) + "M";
  return (x / 1e9).toFixed(2) + "B";
}

export function bytes(b) {
  if (b === null || b === undefined) return "–";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return `${b < 10 && i ? b.toFixed(1) : Math.round(b)} ${u[i]}`;
}

export function ms(x) {
  if (x === null || x === undefined) return "–";
  if (x < 1) return `${x.toFixed(2)} ms`;
  if (x < 100) return `${x.toFixed(1)} ms`;
  if (x < 10000) return `${Math.round(x)} ms`;
  return `${(x / 1000).toFixed(1)} s`;
}

export const pct = (x, digits = 1) => (x === null || x === undefined ? "–" : `${(x * 100).toFixed(digits)}%`);

export function metric(value, unit) {
  if (value === null || value === undefined) return "–";
  switch (unit) {
    case "pct": return pct(value);
    case "ms": return ms(value);
    case "ratio": return value.toFixed(3);
    case "count": return Number.isInteger(value) ? n(value) : value.toFixed(1);
    default: return String(value);
  }
}

const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
export function date(unix) {
  if (!unix) return "–";
  const d = new Date(unix * 1000);
  return `${d.getDate()} ${months[d.getMonth()]} ${d.getFullYear()}`;
}
export function datetime(unix) {
  if (!unix) return "–";
  const d = new Date(unix * 1000);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${d.getDate()} ${months[d.getMonth()]} ${d.getFullYear()}, ${hh}:${mm}`;
}
export function ago(unix) {
  if (!unix) return "";
  const s = Date.now() / 1000 - unix;
  if (s < 90) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400 * 2) return `${Math.round(s / 3600)} h ago`;
  if (s < 86400 * 60) return `${Math.round(s / 86400)} days ago`;
  if (s < 86400 * 730) return `${Math.round(s / (86400 * 30))} months ago`;
  return `${Math.round(s / (86400 * 365))} years ago`;
}

export const short = (sha) => (sha || "").slice(0, 10);

/// A path as dimmed directory plus file name.
export function pathLabel(path) {
  const i = path.lastIndexOf("/");
  return i < 0 ? [h("b", null, path)] : [h("span", { class: "dir" }, path.slice(0, i + 1)), h("b", null, path.slice(i + 1))];
}

/// A path for narrow columns: the file name as the link, the directory
/// underneath, so long paths neither wrap mid-name nor hide the name.
export function pathLink(path, link, extra) {
  const i = path.lastIndexOf("/");
  return h("div", { class: "pathlink", title: path },
    h("a", { href: link }, path.slice(i + 1)),
    extra ? h("span", { class: "extra" }, extra) : null,
    i >= 0 ? h("div", { class: "pdir" }, path.slice(0, i + 1)) : null);
}

/// Text with spans ([start, end] byte offsets into UTF-8) wrapped in <mark>.
export function marked(text, spans) {
  if (!spans || !spans.length || text === null || text === undefined) return [text ?? ""];
  const enc = new TextEncoder().encode(text);
  const dec = new TextDecoder();
  const out = [];
  let at = 0;
  for (const [a, b] of [...spans].sort((x, y) => x[0] - y[0])) {
    if (a < at) continue;
    if (a > at) out.push(dec.decode(enc.slice(at, a)));
    out.push(h("mark", null, dec.decode(enc.slice(a, b))));
    at = b;
  }
  if (at < enc.length) out.push(dec.decode(enc.slice(at)));
  return out;
}

// ── Routing ──────────────────────────────────────────────

/// `#/view?k=v` → { view, params }.
export function parseHash(hash = location.hash) {
  const raw = hash.replace(/^#\/?/, "");
  const [view, query = ""] = raw.split("?");
  return { view: view || "index", params: Object.fromEntries(new URLSearchParams(query)) };
}

export function href(view, params = {}) {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === false || v === "") continue;
    qs.set(k, v === true ? "1" : String(v));
  }
  return `#/${view}${qs.size ? "?" + qs : ""}`;
}

export function go(view, params) {
  location.hash = href(view, params);
}

/// Replace the current history entry: for reflecting input state in the URL
/// without adding a back-button step per keystroke.
export function replace(view, params) {
  history.replaceState(null, "", href(view, params));
}

// ── Small behaviours ─────────────────────────────────────

export function debounce(fn, wait) {
  let t;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), wait);
  };
}

export function toast(text) {
  const el = h("div", { class: "toast", role: "status" }, text);
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 1600);
}

export async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast("Copied");
  } catch {
    toast("Copy failed");
  }
}

export function loading(label = "Loading") {
  return h("div", { class: "loading" }, label);
}

export function errorBox(e) {
  return h("div", { class: "error", role: "alert" }, e.message || String(e));
}

/// A segmented control. `options`: [{value, label, title}]. Calls `onchange`
/// with the new value.
export function segmented(options, value, onchange, cls = "") {
  const root = h("div", { class: `seg ${cls}`, role: "group" });
  const render = (v) => {
    for (const b of root.children) b.setAttribute("aria-pressed", String(b.dataset.value === String(v)));
  };
  for (const o of options) {
    root.appendChild(h("button", {
      type: "button", title: o.title, dataset: { value: String(o.value) }, class: o.mono ? "mono" : "",
      onclick: () => { render(o.value); onchange(o.value); },
    }, o.label));
  }
  render(value);
  root.set = render;
  return root;
}

/// A toggle button inside a .seg.
export function toggle(label, on, onchange, title) {
  const b = h("button", { type: "button", title, "aria-pressed": String(!!on) }, label);
  b.addEventListener("click", () => {
    const next = b.getAttribute("aria-pressed") !== "true";
    b.setAttribute("aria-pressed", String(next));
    onchange(next);
  });
  b.set = (v) => b.setAttribute("aria-pressed", String(!!v));
  return b;
}

/// A sortable table. `columns`: [{key, label, num, render(row), sort(row)}].
export function table(columns, rows, { onclick, initial, selected } = {}) {
  let sortKey = initial?.key ?? null;
  let dir = initial?.dir ?? 1;
  const tbody = h("tbody");
  const headRow = h("tr");
  const thead = h("thead", null, headRow);
  const draw = () => {
    let sorted = rows;
    const col = columns.find((c) => c.key === sortKey);
    if (col) {
      const val = col.sort || ((r) => r[col.key]);
      sorted = [...rows].sort((a, b) => {
        const x = val(a), y = val(b);
        if (x === y) return 0;
        if (x === null || x === undefined) return 1;
        if (y === null || y === undefined) return -1;
        return (x < y ? -1 : 1) * dir;
      });
    }
    clear(tbody, sorted.map((r) => {
      const tr = h("tr", { class: onclick ? "clickable" : "" },
        columns.map((c) => h("td", { class: [c.num ? "num" : "", c.cls || ""].join(" ") }, c.render ? c.render(r) : r[c.key])));
      if (onclick) tr.addEventListener("click", (ev) => onclick(r, ev));
      if (selected && selected(r)) tr.setAttribute("aria-selected", "true");
      return tr;
    }));
    for (const th of headRow.children) {
      th.removeAttribute("aria-sort");
      if (th.dataset.key === sortKey) th.setAttribute("aria-sort", dir > 0 ? "ascending" : "descending");
    }
  };
  for (const c of columns) {
    const th = h("th", { class: [c.num ? "num" : "", c.sortable === false ? "" : "sortable"].join(" "), dataset: { key: c.key }, title: c.title }, c.label);
    if (c.sortable !== false) {
      th.addEventListener("click", () => {
        if (sortKey === c.key) dir = -dir;
        else { sortKey = c.key; dir = c.num ? -1 : 1; }
        draw();
      });
    }
    headRow.appendChild(th);
  }
  draw();
  return h("table", { class: "grid" }, thead, tbody);
}

/// Horizontal bars: rows of {name, count, href?, other?}.
export function barList(rows, { href: link } = {}) {
  const max = Math.max(1, ...rows.map((r) => r.count));
  return h("div", { class: "bars" }, rows.map((r) => [
    link && !r.other ? h("a", { class: "name", href: link(r), title: r.name }, r.name) : h("span", { class: "name", title: r.name }, r.name),
    h("div", { class: "track" }, h("div", { class: `fill ${r.other ? "other" : ""}`, style: { width: `${(r.count / max) * 100}%` } })),
    h("span", { class: "n" }, n(r.count)),
  ]));
}

export function empty(...children) {
  return h("div", { class: "empty" }, ...children);
}

/// Shared page state: the index overview, fetched once and refreshed on demand.
export const state = { overview: null };
