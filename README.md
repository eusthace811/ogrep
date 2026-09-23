# zgrep

An experimental Rust fork of [Microsoft tgrep](https://github.com/microsoft/tgrep), starting from commit `56f8c62`. It provides indexed exact text and regex search with an optional local server.

This fork is under evaluation. Upstream benchmark results and integrations are not evidence of this fork’s performance or adoption. See [the development record](DEVELOPMENT.md) and `evidence/` for reproducible experiments.

```sh
cargo build --release --locked
./target/release/zgrep index .
./target/release/zgrep serve .
./target/release/zgrep -F -- 'fn main' .
```

The executable is named `zgrep`; many Unix systems already have an unrelated gzip search command with that name. Use an explicit path or install this binary under a distinct name such as `zgrep-code` to avoid confusion.

Indexes and service discovery use `.zgrep/`, separate from upstream’s `.tgrep/`. See [agent usage](AGENTS.md) for index freshness and search semantics. Use `--no-index` when searching current filesystem contents is required.

The inherited full command reference and original performance claims are preserved in [upstream documentation](docs/upstream/README.md). Substitute the local executable name and `.zgrep` directory when using those commands. This fork is not distributed through upstream’s download URLs or Homebrew formula.

Original copyright and MIT license are retained in [LICENSE](LICENSE).
