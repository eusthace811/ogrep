// Files: the indexed tree, a file with its outline, and the files it uses and
// is used by, with the names that link them.

import { h, clear, api, aborted, n, bytes, href, pathLink, replace, loading, errorBox, empty, debounce, copy } from "../lib.js";
import { highlight, languageOf } from "../code.js";

const RENDER_CAP = 20000;

let els = {};
// `q`, `fixed`, `case` and `word` are the matching options of the search
// that opened the file, for marking its matches.
let current = { path: null, dir: "", line: null, q: "", fixed: "", case: "", word: "" };
const expanded = new Set([""]);
const listings = new Map();

// ── Tree ─────────────────────────────────────────────────

async function listing(dir) {
  if (!listings.has(dir)) listings.set(dir, api("tree", { dir }));
  return listings.get(dir);
}

async function drawTree() {
  const rows = [];
  const walk = async (dir, depth) => {
    const t = await listing(dir);
    for (const d of t.dirs) {
      const full = `${t.dir}${d.name}/`;
      const open = expanded.has(full);
      rows.push(h("div", { class: "row dir", style: { "--depth": depth }, onclick: () => { open ? expanded.delete(full) : expanded.add(full); drawTree(); } },
        h("span", { class: "twisty" }, open ? "▾" : "▸"), h("span", { class: "nm" }, d.name), h("span", { class: "ct" }, n(d.files))));
      if (open) await walk(full, depth + 1);
    }
    for (const f of t.files) {
      const full = `${t.dir}${f}`;
      rows.push(h("a", { class: "row file", href: href("files", { path: full }), style: { "--depth": depth }, "aria-selected": String(full === current.path) },
        h("span", { class: "twisty" }), h("span", { class: "nm" }, f)));
    }
  };
  try {
    await walk("", 0);
    clear(els.tree, rows);
    els.tree.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  } catch (e) {
    if (!aborted(e)) clear(els.tree, errorBox(e));
  }
}

function revealInTree(path) {
  const parts = path.split("/");
  let acc = "";
  for (const p of parts.slice(0, -1)) {
    acc += `${p}/`;
    expanded.add(acc);
  }
}

const filterFiles = debounce(async (q) => {
  if (!q.trim()) { els.found.hidden = true; els.tree.hidden = false; return; }
  try {
    const r = await api("find", { q, limit: 80 }, { slot: "files-find" });
    els.tree.hidden = true;
    els.found.hidden = false;
    clear(els.found, r.paths.length
      ? r.paths.map((p) => h("a", { class: "row file", href: href("files", { path: p }) }, h("span", { class: "nm" }, p)))
      : h("div", { class: "faint small" }, "No indexed path matches."));
  } catch (e) {
    if (!aborted(e)) clear(els.found, errorBox(e));
  }
}, 150);

// ── File ─────────────────────────────────────────────────

function outlineTree(defs) {
  // Nest by line ranges: a definition inside another's range is its child.
  const stack = [];
  return defs.map((d) => {
    while (stack.length && d.line > stack[stack.length - 1].end_line) stack.pop();
    const depth = stack.length;
    stack.push(d);
    return { ...d, depth };
  });
}

const KIND = { function: "fn", method: "fn", class: "cls", interface: "ifc", module: "mod", macro: "mac", constant: "con", type: "typ", impl: "imp", other: "·" };

function linkList(rows, emptyText) {
  if (!rows.length) return h("p", { class: "faint small" }, emptyText);
  const max = Math.max(...rows.map((r) => r.weight)) || 1;
  return h("div", { class: "links" }, rows.map((r) => h("div", { class: "item" },
    h("span", { class: "w", title: `link weight ${r.weight.toFixed(2)}` }, h("i", { style: { width: `${(r.weight / max) * 100}%` } })),
    pathLink(r.path, href("files", { path: r.path })),
    r.via?.length ? h("div", { class: "via" }, r.via.join("  ")) : null)));
}

/// The URL for the current file at `line`, keeping the search options.
function here(line) {
  const { path, q, fixed, case: cs, word } = current;
  return { path, line, q, fixed, case: cs, word };
}

/// The search's pattern as a JavaScript regex, to mark matches. The dialects
/// agree on everyday patterns; where they do not, nothing is marked.
function queryRegex({ q, fixed, case: cs, word }) {
  if (!q) return null;
  let src = fixed ? q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") : q;
  if (word) src = `(?<!\\w)(?:${src})(?!\\w)`;
  const ci = cs === "insensitive" || (cs !== "sensitive" && !/[A-Z]/.test(q));
  try { return new RegExp(src, ci ? "gi" : "g"); } catch { return null; }
}

/// A line's tokens, with query matches marked on top.
function lineNodes(tokens, re) {
  const nodes = tokens.map(([cls, text]) => (cls ? h("span", { class: cls }, text) : document.createTextNode(text)));
  if (!re) return nodes;
  const text = tokens.map((t) => t[1]).join("");
  re.lastIndex = 0;
  const ranges = [];
  let m;
  while ((m = re.exec(text)) && ranges.length < 50) {
    if (m[0].length === 0) { re.lastIndex++; continue; }
    ranges.push([m.index, m.index + m[0].length]);
  }
  if (!ranges.length) return nodes;
  // Rebuild with marks: walk tokens and split at range edges.
  const out = [];
  let pos = 0;
  for (const [cls, t] of tokens) {
    let i = 0;
    while (i < t.length) {
      const abs = pos + i;
      const r = ranges.find(([a, b]) => abs >= a && abs < b);
      const nextEdge = r ? r[1] : Math.min(...ranges.map(([a]) => a).filter((a) => a > abs), pos + t.length);
      const end = Math.min(nextEdge - pos, t.length);
      const piece = t.slice(i, end);
      const node = cls ? h("span", { class: cls }, piece) : document.createTextNode(piece);
      out.push(r ? h("mark", null, node) : node);
      i = end;
    }
    pos += t.length;
  }
  return out;
}

function renderCode(f) {
  if (f.binary) return empty("Binary file: not shown.");
  const lines = f.text.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  const shown = lines.slice(0, RENDER_CAP);
  const toks = highlight(shown, languageOf(f.path));
  const re = queryRegex(current);
  const code = h("div", { class: "code", role: "table", "aria-label": f.path });
  const frag = document.createDocumentFragment();
  toks.forEach((t, i) => {
    const ln = i + 1;
    frag.appendChild(h("div", { class: "l", id: `L${ln}`, role: "row" },
      h("span", { class: "n", onclick: () => { current.line = ln; replace("files", here(ln)); mark(ln); } }, ln),
      h("span", { class: "t" }, lineNodes(t, re))));
  });
  code.appendChild(frag);
  const notes = [];
  if (lines.length > RENDER_CAP) notes.push(h("div", { class: "note" }, `Showing the first ${n(RENDER_CAP)} of ${n(lines.length)} lines.`));
  if (f.truncated) notes.push(h("div", { class: "note" }, `File is ${bytes(f.bytes)}; showing the first 2 MB.`));
  return [code, notes];
}

function mark(line, range) {
  for (const el of els.code.querySelectorAll(".l.target, .l.inrange")) el.classList.remove("target", "inrange");
  if (!line) return;
  const el = els.code.querySelector(`#L${line}`);
  if (!el) return;
  el.classList.add("target");
  if (range) for (let i = line + 1; i <= range; i++) els.code.querySelector(`#L${i}`)?.classList.add("inrange");
  el.scrollIntoView({ block: "center" });
}

async function openFile(path) {
  clear(els.code, loading());
  clear(els.rail);
  try {
    const f = await api("file", { path }, { slot: "file" });
    const parts = path.split("/");
    const lineCount = f.binary ? 0 : f.text.split("\n").length - (f.text.endsWith("\n") ? 1 : 0);
    clear(els.code,
      h("div", { class: "codehead" },
        h("div", { class: "crumbs" }, parts.map((p, i) => i < parts.length - 1
          ? [h("a", { href: href("files", { dir: parts.slice(0, i + 1).join("/") + "/" }) }, p), "/"]
          : h("b", null, p))),
        h("span", { class: "meta" }, `${n(lineCount)} lines · ${bytes(f.bytes)}`),
        h("span", { class: "actions" },
          f.graph ? h("a", { href: href("graph", { path }) }, "Graph") : null,
          h("a", { href: href("search", { q: path.split("/").pop().replace(/\.[^.]*$/, ""), lens: "matches", fixed: "1" }) }, "Search name"),
          h("a", { href: "#", onclick: (e) => { e.preventDefault(); copy(path); } }, "Copy path"))),
      renderCode(f));
    const outline = outlineTree(f.outline);
    clear(els.rail,
      h("h2", null, "Outline"),
      outline.length
        ? h("div", { class: "outline" }, outline.map((d) => h("a", {
            href: href("files", { path, line: d.line }), style: { "--depth": d.depth }, title: d.signature,
            onclick: (e) => { e.preventDefault(); replace("files", here(d.line)); mark(d.line, d.end_line); },
          }, h("span", { class: "kind" }, KIND[d.kind] || "·"), h("span", { class: "nm" }, d.name), h("span", { class: "ln" }, d.line))))
        : h("p", { class: "faint small" }, f.graph ? "No definitions parsed in this file." : "Needs the file graph: ogrep index --graph."),
      f.graph ? [
        h("h2", null, "Uses"),
        h("p", { class: "faint small" }, "Files defining names this file mentions."),
        linkList(f.uses, "Uses no names defined elsewhere."),
        h("h2", null, "Used by"),
        h("p", { class: "faint small" }, "Files mentioning names this file defines."),
        linkList(f.used_by, "No other file uses its definitions."),
        f.graph_current === false ? h("div", { class: "note" }, "The graph is older than the index; links may be out of date.") : null,
      ] : null);
    const def = f.outline.find((d) => d.line === Number(current.line));
    mark(Number(current.line) || null, def?.end_line);
  } catch (e) {
    if (!aborted(e)) clear(els.code, errorBox(e));
  }
}

function mount(root) {
  const filter = h("input", { class: "field", placeholder: "Filter paths", "aria-label": "Filter paths", "data-primary": "", spellcheck: "false",
    style: { width: "100%" }, oninput: () => filterFiles(filter.value) });
  els = {
    filter,
    tree: h("div", { class: "tree-list", role: "tree" }),
    found: h("div", { class: "tree-list", hidden: true }),
    code: h("div", { class: "pane", style: { paddingTop: 0 } }),
    rail: h("aside", { class: "rail", "aria-label": "Outline and links" }),
  };
  clear(root, h("div", { class: "split three" },
    h("nav", { class: "rail left", "aria-label": "Files" }, h("div", { style: { marginBottom: "8px" } }, filter), els.found, els.tree),
    els.code, els.rail));
}

function route(p) {
  if (!Object.keys(p).length && current.path) { replace("files", here(current.line)); return; }
  const samePath = p.path && p.path === current.path;
  current = { path: p.path || null, dir: p.dir || "", line: p.line || null, q: p.q || "", fixed: p.fixed || "", case: p.case || "", word: p.word || "" };
  if (current.path) revealInTree(current.path);
  if (current.dir) {
    let acc = "";
    for (const part of current.dir.split("/").filter(Boolean)) { acc += `${part}/`; expanded.add(acc); }
  }
  drawTree();
  if (current.path) {
    if (samePath && els.code.querySelector(".code")) mark(Number(current.line) || null);
    else openFile(current.path);
  } else {
    clear(els.code, h("div", { style: { paddingTop: "16px" } },
      empty("Pick a file from the tree, filter by path, or press ", h("kbd", null, "Ctrl K"), " to go to a file or a definition (", h("code", null, "@name"), ").")));
    clear(els.rail);
  }
}

export default { id: "files", title: "Files", mount, route };

