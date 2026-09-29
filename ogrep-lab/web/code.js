// A small, line-oriented syntax highlighter: comments, strings, numbers and
// keywords for the common language families. It keeps state across lines for
// block comments and multi-line strings. Good enough to read by; not a parser.

const KW = {
  py: "and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return self True try while with yield match case",
  rs: "as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while Some None Ok Err",
  js: "async await break case catch class const continue debugger default delete do else export extends false finally for from function if import in instanceof let new null of return static super switch this throw true try typeof undefined var void while with yield interface type enum implements private public protected readonly",
  go: "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var nil true false",
  c: "auto break case char const continue default do double else enum extern float for goto if inline int long register return short signed sizeof static struct switch typedef union unsigned void volatile while class namespace template typename public private protected virtual override new delete this nullptr true false bool using try catch throw final abstract extends implements import package interface null boolean byte string var readonly async await",
  rb: "alias and begin break case class def defined do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield",
  php: "abstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile extends final finally fn for foreach function global goto if implements include instanceof interface isset list match namespace new or print private protected public readonly require return static switch throw trait try unset use var while yield null true false",
  sh: "if then else elif fi for while until do done case esac function in return local export readonly",
};

const C_LIKE = { line: ["//"], block: ["/*", "*/"] };
const FAMILIES = [
  ["py pyi", { kw: "py", line: ["#"], triple: true }],
  ["rs", { kw: "rs", ...C_LIKE }],
  ["js mjs cjs jsx ts tsx", { kw: "js", ...C_LIKE, backtick: true }],
  ["go", { kw: "go", ...C_LIKE, backtick: true }],
  ["c h cc cpp hpp cs java kt scala swift", { kw: "c", ...C_LIKE }],
  ["rb", { kw: "rb", line: ["#"] }],
  ["php", { kw: "php", line: ["//", "#"], block: C_LIKE.block }],
  ["sh bash", { kw: "sh", line: ["#"] }],
  ["yaml yml toml", { line: ["#"] }],
  ["css", { block: C_LIKE.block }],
  ["sql", { line: ["--"], block: C_LIKE.block }],
];
const LANGS = Object.fromEntries(FAMILIES.flatMap(([exts, lang]) => exts.split(" ").map((e) => [e, lang])));

const kwSets = {};
function keywords(name) {
  if (!name) return null;
  return (kwSets[name] ||= new Set(KW[name].split(" ")));
}

export function languageOf(path) {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  if (name === "makefile" || name === "dockerfile") return { line: ["#"] };
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  return LANGS[ext] || null;
}

/// Tokenise lines. Returns, per line, an array of [class|null, text].
export function highlight(lines, lang) {
  if (!lang) return lines.map((l) => [[null, l]]);
  const kw = keywords(lang.kw);
  const out = [];
  let open = null; // an unterminated block comment or multi-line string: its closing delimiter and class
  for (const line of lines) {
    const toks = [];
    let i = 0;
    let plain = "";
    const flush = () => { if (plain) { toks.push([null, plain]); plain = ""; } };
    if (open) {
      const end = line.indexOf(open.close);
      if (end < 0) { out.push([[open.cls, line]]); continue; }
      toks.push([open.cls, line.slice(0, end + open.close.length)]);
      i = end + open.close.length;
      open = null;
    }
    while (i < line.length) {
      const rest = line.slice(i);
      const c = line[i];
      if (lang.line && lang.line.some((p) => rest.startsWith(p))) {
        flush(); toks.push(["tok-com", rest]); i = line.length; break;
      }
      if (lang.block && rest.startsWith(lang.block[0])) {
        flush();
        const end = line.indexOf(lang.block[1], i + lang.block[0].length);
        if (end < 0) { toks.push(["tok-com", rest]); open = { close: lang.block[1], cls: "tok-com" }; i = line.length; break; }
        toks.push(["tok-com", line.slice(i, end + lang.block[1].length)]); i = end + lang.block[1].length; continue;
      }
      if (lang.triple && (rest.startsWith('"""') || rest.startsWith("'''"))) {
        flush();
        const q = rest.slice(0, 3);
        const end = line.indexOf(q, i + 3);
        if (end < 0) { toks.push(["tok-str", rest]); open = { close: q, cls: "tok-str" }; i = line.length; break; }
        toks.push(["tok-str", line.slice(i, end + 3)]); i = end + 3; continue;
      }
      if (c === '"' || c === "'" || (c === "`" && lang.backtick)) {
        // A Rust lifetime ('a) is not a string.
        if (c === "'" && lang.kw === "rs" && /^'[A-Za-z_]\w*(?!')/.test(rest) && !/^'.'/.test(rest)) { plain += c; i++; continue; }
        flush();
        let j = i + 1;
        while (j < line.length && line[j] !== c) j += line[j] === "\\" ? 2 : 1;
        if (j >= line.length && c === "`") { toks.push(["tok-str", rest]); open = { close: "`", cls: "tok-str" }; i = line.length; break; }
        toks.push(["tok-str", line.slice(i, j + 1)]); i = j + 1; continue;
      }
      if (/[0-9]/.test(c) && !/[\w$]/.test(line[i - 1] || "")) {
        const m = /^(0x[0-9a-fA-F_]+|\d[\d_]*(\.\d+)?([eE][+-]?\d+)?)[a-zA-Z0-9_]*/.exec(rest);
        flush(); toks.push(["tok-num", m[0]]); i += m[0].length; continue;
      }
      if (/[A-Za-z_$]/.test(c)) {
        const m = /^[A-Za-z_$][\w$]*/.exec(rest);
        if (kw && kw.has(m[0])) { flush(); toks.push(["tok-kw", m[0]]); } else plain += m[0];
        i += m[0].length; continue;
      }
      plain += c; i++;
    }
    flush();
    out.push(toks);
  }
  return out;
}
