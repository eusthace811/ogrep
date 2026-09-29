// Index: what is built, how big each part is, and the shape of the tree.

import { h, clear, api, n, compact, bytes, pct, date, datetime, ago, short, barList, href, empty, loading, errorBox, state, aborted } from "../lib.js";

const SWATCH = { postings: "--s1", lookup: "--s2", paths: "--s3", evidence: "--s4", history: "--s5", graph: "--s6", facts: "--s9", lab: "--s7", other: "--s8" };

let root;

function stat(value, label, unit) {
  return h("div", { class: "stat" }, h("div", { class: "v" }, value, unit ? h("small", null, unit) : null), h("div", { class: "k" }, label));
}

function render(o) {
  const m = o.meta;
  const total = o.storage.reduce((a, s) => a + s.bytes, 0);
  const keys = m.trigram_keys + (m.sparse_gram_keys || 0);

  const storage = [
    h("div", { class: "stack", role: "img", "aria-label": "Index size by part" },
      o.storage.map((s) => h("span", { title: `${s.part}: ${bytes(s.bytes)}`, style: { width: `${(s.bytes / total) * 100}%`, background: `var(${SWATCH[s.part]})` } }))),
    h("table", { class: "grid" },
      h("thead", null, h("tr", null, h("th", null, "Part"), h("th", { class: "num" }, "Size"), h("th", { class: "num" }, "Share"), h("th", null, "Holds"))),
      h("tbody", null, o.storage.map((s) => h("tr", null,
        h("td", null, h("span", { class: "swatch", style: { background: `var(${SWATCH[s.part]})` } }), s.part),
        h("td", { class: "num" }, bytes(s.bytes)),
        h("td", { class: "num" }, pct(s.bytes / total)),
        h("td", { class: "muted" }, s.what)))),
      h("tfoot", null, h("tr", null, h("td", null, h("b", null, "Total")), h("td", { class: "num" }, h("b", null, bytes(total))), h("td"), h("td", { class: "faint" }, String(o.index_dir))))),
  ];

  const history = o.history
    ? h("dl", { class: "dl" },
        h("dt", null, "Commits"), h("dd", null, n(o.history.commits), o.history.truncated ? h("span", { class: "faint" }, ` (${n(o.history.truncated)} cut at the size cap)`) : null),
        h("dt", null, "Covers"), h("dd", null, `${date(o.history.oldest)} to ${date(o.history.newest)}`),
        h("dt", null, "Head"), h("dd", { class: "mono" }, short(o.history.head)),
        h("dt", null, "Limit"), h("dd", null, o.history.depth ? `${n(o.history.depth)} commits` : o.history.since ? `since ${o.history.since}` : "full history"),
        h("dt", null, ""), h("dd", null, h("a", { href: href("history") }, "Search history")))
    : empty("Not built. ", h("code", null, "ogrep index --history"), " indexes every commit's added and removed lines, for ", h("code", null, "--history"), " and ", h("code", null, "--introduced"), ".");

  const graph = o.graph
    ? h("dl", { class: "dl" },
        h("dt", null, "Files"), h("dd", null, n(o.graph.files)),
        h("dt", null, "Links"), h("dd", null, n(o.graph.links)),
        h("dt", null, "Definitions"), h("dd", null, n(o.graph.definitions)),
        h("dt", null, "Built"), h("dd", null, o.graph.built_at ? `${datetime(o.graph.built_at)} (${ago(o.graph.built_at)})` : "–"),
        h("dt", null, "State"), h("dd", null, o.graph.current
          ? h("span", { class: "pill ok" }, "matches the index")
          : [h("span", { class: "pill warn" }, "older than the index"), " ", h("code", { class: "small" }, "ogrep index --graph"), h("span", { class: "faint small" }, " refreshes it")]),
        h("dt", null, ""), h("dd", null, h("a", { href: href("graph") }, "Explore the graph")))
    : empty("Not built. ", h("code", null, "ogrep index --graph"), " records definitions and which files use them, for the outline, ", h("code", null, "--near"), ", ", h("code", null, "--map"), " and the graph view.");

  clear(root, h("div", { class: "page" },
    h("div", { class: "stats" },
      stat(n(m.files), "files indexed"),
      stat(compact(m.trigram_keys), "trigram keys"),
      m.sparse_grams ? stat(compact(m.sparse_gram_keys || 0), `sparse-gram keys, v${m.sparse_grams}`) : stat("off", "sparse grams"),
      stat(bytes(total), "on disk"),
      stat(bytes(total / Math.max(1, m.files)), "per file"),
      stat(ago(m.updated_at), `updated ${datetime(m.updated_at)}`),
    ),
    !m.complete ? h("div", { class: "note" }, "This index is partial: the server stopped during background indexing. Searches are still correct but scan what the index lacks.") : null,
    !m.sparse_grams ? h("div", { class: "note" }, "Built without sparse grams (or merged onto an index without them). Queries fall back to trigrams. A full ", h("code", null, "ogrep index"), " adds them.") : null,

    h("h2", null, "Storage"),
    h("p", { class: "prose small" }, `${n(keys)} keys in all. Postings are the lists of files per key; lookup maps each key to its list.`),
    storage,

    h("div", { class: "cols section" },
      h("div", null, h("h2", null, "History"), history),
      h("div", null, h("h2", null, "File graph"), graph),
    ),

    h("div", { class: "cols section" },
      h("div", null, h("h2", null, "File types"), barList(o.file_types)),
      h("div", null, h("h2", null, "Top-level directories"),
        barList(o.directories, { href: (r) => href("files", { dir: r.name.endsWith("/") ? r.name : "" }) })),
    ),

    h("h2", { class: "section" }, "Tools"),
    h("dl", { class: "dl" },
      h("dt", null, "Repository"), h("dd", { class: "mono" }, String(o.root)),
      h("dt", null, "ogrep"), h("dd", { class: "mono" }, o.ogrep ? `${o.ogrep} (${o.ogrep_version || "version unknown"})` : "not found: search, history and evaluations are unavailable"),
      h("dt", null, "ogrep-lab"), h("dd", { class: "mono" }, o.lab_version),
      h("dt", null, "Index format"), h("dd", null, `v${m.format}, created ${datetime(m.created_at)}`),
    ),
  ));
}

export default {
  id: "index",
  title: "Index",
  mount(el) {
    root = el;
    clear(root, h("div", { class: "page" }, loading()));
  },
  async route() {
    try {
      const o = await api("overview", {}, { slot: "overview" });
      state.overview = o;
      render(o);
    } catch (e) {
      if (!aborted(e)) clear(root, h("div", { class: "page" }, errorBox(e)));
    }
  },
};
