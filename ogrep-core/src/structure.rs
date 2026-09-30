//! Source structure: where definitions are, how they nest, and which bytes are
//! comments or string literals.
//!
//! Used for `--scope` (the definition enclosing each match), `--kind` (whether
//! a match defines or uses a name), `--in` (code, comment or string) and to
//! recognise definition lines when ranking. Files are parsed with tree-sitter
//! grammars compiled into the binary, and definitions are recognised by node
//! kind in the same single walk of the tree that finds comments and strings.
//! (Each grammar's tags query finds the same definitions, but compiling one
//! costs about 5 ms per process, more than parsing a typical file.) Files in
//! other languages have no structure, and the features that need it pass them
//! through unchanged.

use tree_sitter::{Language, Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Interface,
    Module,
    Macro,
    Constant,
    Type,
    Impl,
    Other,
}

/// A definition in a file. Byte offsets are into the text that was analysed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    pub start: usize,
    pub end: usize,
    /// 1-based line of the definition's first byte.
    pub start_line: usize,
    pub end_line: usize,
    pub name_start: usize,
    pub name_end: usize,
    /// Index of the innermost symbol containing this one.
    pub parent: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    Code,
    Comment,
    String,
}

#[derive(Clone, Debug, Default)]
pub struct FileStructure {
    pub language: &'static str,
    /// Sorted by start offset; a parent precedes its children.
    pub symbols: Vec<Symbol>,
    /// Non-overlapping comment and string ranges, sorted by start.
    zones: Vec<(usize, usize, Zone)>,
}

impl FileStructure {
    /// The symbols containing `offset`, outermost first.
    pub fn enclosing(&self, offset: usize) -> Vec<&Symbol> {
        let mut innermost = None;
        for (i, s) in self.symbols.iter().enumerate() {
            if s.start > offset {
                break;
            }
            if offset < s.end {
                innermost = Some(i);
            }
        }
        let mut chain = Vec::new();
        let mut at = innermost;
        while let Some(i) = at {
            chain.push(&self.symbols[i]);
            at = self.symbols[i].parent;
        }
        chain.reverse();
        chain
    }

    /// Whether `offset` is in a comment, a string literal, or code.
    pub fn zone_at(&self, offset: usize) -> Zone {
        let i = self.zones.partition_point(|&(start, _, _)| start <= offset);
        match i.checked_sub(1).map(|i| self.zones[i]) {
            Some((_, end, zone)) if offset < end => zone,
            _ => Zone::Code,
        }
    }

    /// Serialise for the fact cache ([`crate::facts`]).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.symbols.len() * 40 + self.zones.len() * 9);
        let put = |out: &mut Vec<u8>, v: usize| {
            out.extend_from_slice(&(v.min(u32::MAX as usize) as u32).to_le_bytes())
        };
        out.push(
            LANGUAGES
                .iter()
                .position(|l| *l == self.language)
                .unwrap_or(0) as u8,
        );
        put(&mut out, self.symbols.len());
        for s in &self.symbols {
            out.push(s.kind as u8);
            for v in [
                s.start,
                s.end,
                s.start_line,
                s.end_line,
                s.name_start,
                s.name_end,
            ] {
                put(&mut out, v);
            }
            put(&mut out, s.parent.unwrap_or(u32::MAX as usize));
            put(&mut out, s.name.len());
            out.extend_from_slice(s.name.as_bytes());
        }
        put(&mut out, self.zones.len());
        for &(start, end, zone) in &self.zones {
            put(&mut out, start);
            put(&mut out, end);
            out.push(zone as u8);
        }
        out
    }

    /// Inverse of [`Self::encode`]; `None` for a malformed payload.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        struct Cursor<'a>(&'a [u8]);
        impl<'a> Cursor<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> {
                let (head, tail) = self.0.split_at_checked(n)?;
                self.0 = tail;
                Some(head)
            }
            fn byte(&mut self) -> Option<u8> {
                Some(self.take(1)?[0])
            }
            fn num(&mut self) -> Option<usize> {
                let raw = u32::from_le_bytes(self.take(4)?.try_into().ok()?);
                Some(if raw == u32::MAX {
                    usize::MAX
                } else {
                    raw as usize
                })
            }
        }
        let mut c = Cursor(bytes);
        let language = *LANGUAGES.get(usize::from(c.byte()?))?;
        let n = c.num()?;
        let mut symbols = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            let kind = KINDS.get(usize::from(c.byte()?)).copied()?;
            let (start, end, start_line, end_line, name_start, name_end) =
                (c.num()?, c.num()?, c.num()?, c.num()?, c.num()?, c.num()?);
            let parent = c.num()?;
            // A parent precedes its children; anything else is corrupt, and
            // would make `enclosing` index out of bounds or loop.
            if parent != usize::MAX && parent >= symbols.len() {
                return None;
            }
            let len = c.num()?;
            let name = String::from_utf8(c.take(len)?.to_vec()).ok()?;
            symbols.push(Symbol {
                kind,
                name,
                start,
                end,
                start_line,
                end_line,
                name_start,
                name_end,
                parent: (parent != usize::MAX).then_some(parent),
            });
        }
        let z = c.num()?;
        let mut zones = Vec::with_capacity(z.min(1 << 22));
        for _ in 0..z {
            let (start, end) = (c.num()?, c.num()?);
            let zone = match c.byte()? {
                0 => Zone::Code,
                1 => Zone::Comment,
                _ => Zone::String,
            };
            zones.push((start, end, zone));
        }
        Some(Self {
            language,
            symbols,
            zones,
        })
    }

    /// Whether `start..end` covers the whole name of a definition: the match is
    /// where that name is defined. Part of a longer name does not count:
    /// `bulk_update` inside `def test_bulk_update` defines something else.
    pub fn defines(&self, start: usize, end: usize) -> bool {
        self.symbols
            .iter()
            .any(|s| start <= s.name_start && s.name_end <= end)
    }
}

/// Language names in a fixed order, for [`FileStructure::encode`].
const LANGUAGES: &[&str] = &[
    "rust",
    "python",
    "go",
    "javascript",
    "typescript",
    "tsx",
    "java",
    "c",
    "cpp",
    "csharp",
    "ruby",
    "php",
];

/// Symbol kinds in declaration order, for [`FileStructure::decode`].
const KINDS: &[SymbolKind] = &[
    SymbolKind::Function,
    SymbolKind::Method,
    SymbolKind::Class,
    SymbolKind::Interface,
    SymbolKind::Module,
    SymbolKind::Macro,
    SymbolKind::Constant,
    SymbolKind::Type,
    SymbolKind::Impl,
    SymbolKind::Other,
];

struct Grammar {
    name: &'static str,
    language: fn() -> Language,
}

fn grammar_for(path: &str) -> Option<&'static Grammar> {
    static GRAMMARS: &[Grammar] = &[
        Grammar {
            name: "rust",
            language: || tree_sitter_rust::LANGUAGE.into(),
        },
        Grammar {
            name: "python",
            language: || tree_sitter_python::LANGUAGE.into(),
        },
        Grammar {
            name: "go",
            language: || tree_sitter_go::LANGUAGE.into(),
        },
        Grammar {
            name: "javascript",
            language: || tree_sitter_javascript::LANGUAGE.into(),
        },
        Grammar {
            name: "typescript",
            language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        },
        Grammar {
            name: "tsx",
            language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
        },
        Grammar {
            name: "java",
            language: || tree_sitter_java::LANGUAGE.into(),
        },
        Grammar {
            name: "c",
            language: || tree_sitter_c::LANGUAGE.into(),
        },
        Grammar {
            name: "cpp",
            language: || tree_sitter_cpp::LANGUAGE.into(),
        },
        Grammar {
            name: "csharp",
            language: || tree_sitter_c_sharp::LANGUAGE.into(),
        },
        Grammar {
            name: "ruby",
            language: || tree_sitter_ruby::LANGUAGE.into(),
        },
        Grammar {
            name: "php",
            language: || tree_sitter_php::LANGUAGE_PHP.into(),
        },
    ];
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    let name = match ext.as_str() {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "go" => "go",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "java" => "java",
        "c" => "c",
        // `.h` is shared by C and C++; the C++ grammar parses both well enough
        // to find definitions.
        "h" | "hh" | "hpp" | "hxx" | "cc" | "cpp" | "cxx" | "c++" | "ipp" | "tpp" => "cpp",
        "cs" => "csharp",
        "rb" | "rake" | "gemspec" => "ruby",
        "php" | "phtml" => "php",
        _ => return None,
    };
    GRAMMARS.iter().find(|g| g.name == name)
}

/// Whether `path` is in a language with structure support.
pub fn supports(path: &str) -> bool {
    grammar_for(path).is_some()
}

fn is_string_kind(kind: &str) -> bool {
    kind.contains("string")
        || matches!(
            kind,
            "char_literal"
                | "character_literal"
                | "heredoc_body"
                | "template_string"
                | "rune_literal"
        )
}

/// Text of a node's field, following nested declarators down to the name, as C
/// and C++ wrap a function's name in pointer, reference and function
/// declarators.
fn declarator_name<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut at = node.child_by_field_name("declarator")?;
    loop {
        match at.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
            | "operator_name" => return Some(at),
            // `Store::save`: the name is the last part.
            "qualified_identifier" => at = at.child_by_field_name("name")?,
            // Reference and parenthesized declarators (`&name()`, `(*cb)`)
            // hold theirs as a plain child, not a `declarator` field.
            _ => {
                at = at
                    .child_by_field_name("declarator")
                    .or_else(|| at.named_child(0))?
            }
        }
    }
}

/// Whether `node` defines something, and its kind and name node.
fn definition<'t>(
    language: &str,
    node: Node<'t>,
    parent_kind: &str,
) -> Option<(SymbolKind, Node<'t>)> {
    use SymbolKind::*;
    let named = |kind| node.child_by_field_name("name").map(|n| (kind, n));
    let in_class = matches!(
        parent_kind,
        "impl_item"
            | "trait_item"
            | "declaration_list"
            | "class_body"
            | "class_declaration"
            | "block"
            | "body_statement"
            | "field_declaration_list"
    );
    // A body's own parent decides: a Rust `declaration_list` is a method
    // body under `impl`/`trait` but not in `mod {}` or `extern {}`; a Python
    // method's block belongs to a class, with a decorator in between.
    let grandparent = || Some(node.parent()?.parent()?.kind());
    match (language, node.kind()) {
        ("rust", "function_item" | "function_signature_item") => named(
            if parent_kind == "declaration_list"
                && matches!(grandparent(), Some("impl_item" | "trait_item"))
            {
                Method
            } else {
                Function
            },
        ),
        ("rust", "struct_item" | "enum_item" | "union_item" | "type_item") => named(Type),
        ("rust", "trait_item") => named(Interface),
        ("rust", "mod_item") => named(Module),
        ("rust", "const_item" | "static_item") => named(Constant),
        ("rust", "macro_definition") => named(Macro),
        ("rust", "impl_item") => node.child_by_field_name("type").map(|n| (Impl, n)),
        ("python", "function_definition") => {
            let body = {
                let mut up = node.parent();
                while up.is_some_and(|p| p.kind() == "decorated_definition") {
                    up = up.and_then(|p| p.parent());
                }
                up
            };
            let method = body.is_some_and(|b| {
                b.kind() == "block" && b.parent().is_some_and(|c| c.kind() == "class_definition")
            });
            named(if method { Method } else { Function })
        }
        ("python", "class_definition") => named(Class),
        ("go", "function_declaration") => named(Function),
        ("go", "method_declaration") => named(Method),
        ("go", "type_spec") => named(Type),
        ("javascript" | "typescript" | "tsx", kind) => match kind {
            "function_declaration" | "generator_function_declaration" | "function_signature" => {
                named(Function)
            }
            "method_definition" | "method_signature" | "abstract_method_signature" => named(Method),
            "class_declaration" | "abstract_class_declaration" | "class" => named(Class),
            "interface_declaration" => named(Interface),
            "type_alias_declaration" | "enum_declaration" => named(Type),
            "module" | "internal_module" => named(Module),
            // `const handler = () => ...`: a function by another spelling.
            "variable_declarator" => {
                let value = node.child_by_field_name("value")?;
                matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "function"
                )
                .then_some(())?;
                named(Function)
            }
            _ => None,
        },
        ("java", kind) => match kind {
            "class_declaration" | "record_declaration" => named(Class),
            "interface_declaration" | "annotation_type_declaration" => named(Interface),
            "enum_declaration" => named(Type),
            "method_declaration" | "constructor_declaration" => named(Method),
            _ => None,
        },
        ("c" | "cpp", kind) => match kind {
            "function_definition" => declarator_name(node).map(|n| (Function, n)),
            "struct_specifier" | "union_specifier" | "enum_specifier" | "class_specifier" => {
                // Only a definition when it has a body.
                node.child_by_field_name("body")?;
                named(if kind == "class_specifier" {
                    Class
                } else {
                    Type
                })
            }
            "type_definition" => declarator_name(node).map(|n| (Type, n)),
            "namespace_definition" => named(Module),
            _ => None,
        },
        ("csharp", kind) => match kind {
            "class_declaration" | "record_declaration" => named(Class),
            "struct_declaration" | "enum_declaration" => named(Type),
            "interface_declaration" => named(Interface),
            "method_declaration" | "constructor_declaration" => named(Method),
            "namespace_declaration" | "file_scoped_namespace_declaration" => named(Module),
            _ => None,
        },
        ("ruby", kind) => match kind {
            "method" | "singleton_method" => named(if in_class { Method } else { Function }),
            "class" => named(Class),
            "module" => named(Module),
            _ => None,
        },
        ("php", kind) => match kind {
            "function_definition" => named(Function),
            "method_declaration" => named(Method),
            "class_declaration" | "trait_declaration" => named(Class),
            "interface_declaration" => named(Interface),
            "enum_declaration" => named(Type),
            _ => None,
        },
        _ => None,
    }
}

/// `path`'s structure: from `cache` when this exact text was parsed before by
/// the same grammar, otherwise parsed now, with the entry to add to the cache
/// (only when there is a cache). Entries are keyed by grammar and content:
/// the same bytes in `util.js` and `util.ts` parse differently.
pub fn analyze_cached(
    path: &str,
    text: &[u8],
    cache: Option<&crate::facts::FactStore>,
) -> (Option<FileStructure>, Option<crate::facts::NewFact>) {
    let Some(grammar) = grammar_for(path) else {
        return (None, None);
    };
    let id = crate::meta::ContentId::of_derived(grammar.name.as_bytes(), text);
    if let Some(found) = cache
        .and_then(|c| c.get(&id))
        .and_then(FileStructure::decode)
    {
        return (Some(found), None);
    }
    let parsed = analyze(path, text);
    let fact = cache.and(parsed.as_ref()).map(|p| (id, p.encode()));
    (parsed, fact)
}

/// Analyse a file's text. `None` for an unsupported language or unparsable
/// input.
pub fn analyze(path: &str, text: &[u8]) -> Option<FileStructure> {
    let grammar = grammar_for(path)?;
    let mut parser = Parser::new();
    parser.set_language(&(grammar.language)()).ok()?;
    let tree = parser.parse(text, None)?;

    let mut symbols: Vec<Symbol> = Vec::new();
    let mut zones = Vec::new();
    let mut walk = tree.root_node().walk();
    let mut parents: Vec<&'static str> = vec![""];
    'walk: loop {
        let node = walk.node();
        let kind = node.kind();
        let mut descend = true;
        // Named nodes only: TypeScript's and PHP's `string` type keyword is an
        // anonymous token, not a string literal.
        if node.is_named() && kind.contains("comment") {
            zones.push((node.start_byte(), node.end_byte(), Zone::Comment));
            descend = false;
        } else if node.is_named() && is_string_kind(kind) {
            zones.push((node.start_byte(), node.end_byte(), Zone::String));
            descend = false;
        } else if let Some((sym_kind, name)) =
            definition(grammar.name, node, parents.last().copied().unwrap_or(""))
        {
            let (name_start, name_end) = if sym_kind == SymbolKind::Impl {
                // An impl names a type defined elsewhere.
                (usize::MAX, usize::MAX)
            } else {
                (name.start_byte(), name.end_byte())
            };
            symbols.push(Symbol {
                kind: sym_kind,
                name: String::from_utf8_lossy(&text[name.byte_range()]).into_owned(),
                start: node.start_byte(),
                end: node.end_byte(),
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                name_start,
                name_end,
                parent: None,
            });
        }
        if descend && walk.goto_first_child() {
            parents.push(kind);
            continue;
        }
        while !walk.goto_next_sibling() {
            if !walk.goto_parent() {
                break 'walk;
            }
            parents.pop();
        }
    }

    // Parents: the innermost earlier symbol still open around each one. The
    // walk visits symbols in document order, outer before inner.
    let mut open: Vec<usize> = Vec::new();
    for i in 0..symbols.len() {
        while let Some(&top) = open.last() {
            if symbols[top].end >= symbols[i].end && symbols[top].start <= symbols[i].start {
                break;
            }
            open.pop();
        }
        symbols[i].parent = open.last().copied();
        open.push(i);
    }
    zones.sort_unstable_by_key(|&(start, _, _)| start);
    Some(FileStructure {
        language: grammar.name,
        symbols,
        zones,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_kind_names_follow_symbol_kinds() {
        for (i, kind) in KINDS.iter().enumerate() {
            assert_eq!(*kind as usize, i, "KINDS is in SymbolKind order");
            let name = serde_json::to_value(kind).unwrap();
            assert_eq!(crate::graph::kind_name(i as u8), name.as_str().unwrap());
        }
    }

    #[test]
    fn rust_symbols_nest_and_zones_are_found() {
        let src = br#"// top comment
pub struct QuerySet { n: u32 }
impl QuerySet {
    /// Docs mention bulk_update.
    pub fn bulk_update(&self) -> &str {
        let s = "bulk_update in a string";
        s
    }
}
fn caller() { QuerySet{n:1}.bulk_update(); }
"#;
        let fs = analyze("x.rs", src).expect("rust is supported");
        let text = std::str::from_utf8(src).unwrap();
        let def = text.find("fn bulk_update").unwrap() + 3;
        let chain: Vec<(&str, SymbolKind)> = fs
            .enclosing(def)
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert_eq!(
            chain,
            vec![
                ("QuerySet", SymbolKind::Impl),
                ("bulk_update", SymbolKind::Method)
            ]
        );
        assert!(fs.defines(def, def + "bulk_update".len()));
        let call = text.rfind("bulk_update").unwrap();
        assert!(!fs.defines(call, call + 11));
        // Part of a longer name is not its definition.
        assert!(!fs.defines(def + 1, def + "bulk_update".len()));
        assert_eq!(fs.enclosing(call).last().unwrap().name, "caller");
        assert_eq!(
            fs.zone_at(text.find("Docs mention").unwrap()),
            Zone::Comment
        );
        assert_eq!(fs.zone_at(text.find("in a string").unwrap()), Zone::String);
        assert_eq!(fs.zone_at(call), Zone::Code);
        assert_eq!(fs.zone_at(1), Zone::Comment);
    }

    #[test]
    fn kinds_names_and_zones_in_awkward_places() {
        let kinds = |path: &str, src: &str| -> Vec<(String, SymbolKind)> {
            analyze(path, src.as_bytes())
                .unwrap()
                .symbols
                .iter()
                .map(|s| (s.name.clone(), s.kind))
                .collect()
        };
        // A free function in an inline module is not a method.
        let rust = kinds(
            "x.rs",
            "mod tests { fn helper() {} }\nimpl A { fn m(&self) {} }\n",
        );
        assert!(
            rust.contains(&("helper".into(), SymbolKind::Function)),
            "{rust:?}"
        );
        assert!(rust.contains(&("m".into(), SymbolKind::Method)), "{rust:?}");
        // A decorated method is a method; a nested function is not.
        let py = kinds(
            "x.py",
            "class A:\n    @property\n    def x(self):\n        def inner():\n            pass\n",
        );
        assert!(py.contains(&("x".into(), SymbolKind::Method)), "{py:?}");
        assert!(
            py.contains(&("inner".into(), SymbolKind::Function)),
            "{py:?}"
        );
        // Out-of-line and reference-returning C++ definitions are named.
        let cpp = kinds(
            "x.cpp",
            "void Store::save() {}\nconst std::string& name() { return s; }\n",
        );
        assert!(cpp.iter().any(|(n, _)| n == "save"), "{cpp:?}");
        assert!(cpp.iter().any(|(n, _)| n == "name"), "{cpp:?}");
        // TypeScript's `string` type keyword is code, not a string literal.
        let ts = "let s: string = x;\n";
        let fs = analyze("x.ts", ts.as_bytes()).unwrap();
        assert_eq!(fs.zone_at(ts.find("string").unwrap()), Zone::Code);
    }

    #[test]
    fn decode_rejects_a_parent_that_does_not_precede_its_child() {
        let mut fs = analyze("x.rs", b"fn a() {}\n").unwrap();
        fs.symbols[0].parent = Some(0);
        assert!(FileStructure::decode(&fs.encode()).is_none());
    }

    #[test]
    fn python_classes_and_methods() {
        let src = b"class QuerySet:\n    def bulk_update(self, objs):\n        # bulk_update comment\n        return 'bulk_update'\n";
        let fs = analyze("query.py", src).unwrap();
        let text = std::str::from_utf8(src).unwrap();
        let def = text.find("bulk_update").unwrap();
        let chain: Vec<&str> = fs.enclosing(def).iter().map(|s| s.name.as_str()).collect();
        assert_eq!(chain, vec!["QuerySet", "bulk_update"]);
        assert!(fs.defines(def, def + 11));
        assert_eq!(
            fs.zone_at(text.find("bulk_update comment").unwrap()),
            Zone::Comment
        );
        assert_eq!(fs.zone_at(text.rfind("bulk_update").unwrap()), Zone::String);
        assert_eq!(fs.symbols[1].start_line, 2);
    }

    /// One sample per remaining language: the chain enclosing a marker inside
    /// a method body, outermost first.
    #[test]
    fn every_language_finds_nested_definitions() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "a.go",
                "package p\ntype Store struct{}\nfunc (s *Store) Save() {\n\tMARK()\n}\n",
                &["Save"],
            ),
            (
                "a.js",
                "class Store {\n  save() {\n    MARK();\n  }\n}\nconst h = () => 1;\n",
                &["Store", "save"],
            ),
            (
                "a.ts",
                "interface I { x: number }\nclass Store {\n  save(): void {\n    MARK();\n  }\n}\n",
                &["Store", "save"],
            ),
            (
                "A.java",
                "class Store {\n  void save() {\n    MARK();\n  }\n}\n",
                &["Store", "save"],
            ),
            (
                "a.c",
                "struct s { int x; };\nstatic int *save(int a) {\n  MARK();\n}\n",
                &["save"],
            ),
            (
                "a.cc",
                "namespace db {\nclass Store {\n  void save() {\n    MARK();\n  }\n};\n}\n",
                &["db", "Store", "save"],
            ),
            (
                "a.cs",
                "namespace Db {\nclass Store {\n  void Save() {\n    MARK();\n  }\n}\n}\n",
                &["Db", "Store", "Save"],
            ),
            (
                "a.rb",
                "module Db\n  class Store\n    def save\n      MARK()\n    end\n  end\nend\n",
                &["Db", "Store", "save"],
            ),
            (
                "a.php",
                "<?php\nclass Store {\n  function save() {\n    MARK();\n  }\n}\n",
                &["Store", "save"],
            ),
        ];
        for (path, src, want) in cases {
            let fs = analyze(path, src.as_bytes()).unwrap_or_else(|| panic!("{path} parses"));
            let at = src.find("MARK").unwrap();
            let chain: Vec<&str> = fs.enclosing(at).iter().map(|s| s.name.as_str()).collect();
            assert_eq!(&chain, want, "{path}");
        }
    }

    #[test]
    fn encoding_round_trips() {
        let src = b"impl A {\n    fn b() { let s = \"x\"; } // c\n}\nfn d() {}\n";
        let fs = analyze("x.rs", src).unwrap();
        let back = FileStructure::decode(&fs.encode()).unwrap();
        assert_eq!(back.language, fs.language);
        assert_eq!(back.symbols, fs.symbols);
        assert_eq!(back.zones, fs.zones);
        assert!(FileStructure::decode(&fs.encode()[..5]).is_none());
    }

    #[test]
    fn unsupported_languages_have_no_structure() {
        assert!(analyze("notes.txt", b"fn x() {}").is_none());
        assert!(supports("a/b.tsx") && supports("x.cc") && !supports("Makefile"));
    }
}
