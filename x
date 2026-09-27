#!/usr/bin/env bash
# Windows build / test / install helper. On Linux use make (make release,
# make install); this script is for the two ways of producing ogrep.exe:
#
#   Git Bash  Native cargo. On the work laptop the repo sits on a Windows
#             disk and nothing special is needed. If the repo is on the WSL
#             share instead (//wsl.localhost/...), target/ has to go to a
#             Windows-local directory: link.exe reads the leading / of a
#             //wsl.localhost object path as an option (LNK4044, then
#             LNK1561), and the 9p share has no file locks for cargo.
#   WSL       The source lives here but the compiler is the Windows MSVC
#             toolchain (cargo.exe), for the same target/ reason, and
#             because WSL forwards env vars to Windows only through WSLENV.
#
#   ./x fetch              download every locked crate (first thing to run on
#                          a corporate network: shows Artifactory serves them)
#   ./x build | release    debug or release build of ogrep.exe
#   ./x install [dir]      release build, then copy ogrep.exe to dir
#                          (default ~/bin, which Git Bash has on PATH)
#   ./x test [args]        cargo test --workspace --locked
#   ./x vendor             cargo vendor into ./vendor, for offline builds
#   ./x doctor             where cargo, the build and the crates come from
#   ./x <anything else>    passed to cargo (clippy, fmt, tree, ...)
#
# Registry: cargo uses the machine's own ~/.cargo/config.toml, which on a
# corporate laptop normally already replaces crates.io with Artifactory. To
# point one run somewhere without touching that file, set
#   OGREP_CARGO_REGISTRY=sparse+https://<host>/artifactory/api/cargo/<repo>/index/
# and, if it needs a token, CARGO_REGISTRIES_OGREP_MIRROR_TOKEN.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$(uname -s)" in
  MINGW*|MSYS*) HOST=win ;;
  Linux)        if grep -qi microsoft /proc/version 2>/dev/null; then HOST=wsl; else HOST=linux; fi ;;
  *)            HOST=other ;;
esac

if [ "$HOST" = linux ] || [ "$HOST" = other ]; then
  echo "x: this script builds the Windows ogrep.exe. On this system use make:" >&2
  echo "     make release     # target/release/ogrep" >&2
  echo "     make install     # ~/.cargo/bin/ogrep" >&2
  echo "     make test" >&2
  exit 2
fi

if [ "$HOST" = wsl ]; then
  # Ask Windows where home is rather than hardcoding a user name.
  WINHOME_W="${OGREP_WINHOME:-$(cd /mnt/c && cmd.exe /c 'echo %USERPROFILE%' 2>/dev/null | tr -d '\r\n')}"
  if [ -z "$WINHOME_W" ]; then
    echo "x: cannot find the Windows home directory; set OGREP_WINHOME='C:\\Users\\you'" >&2
    exit 1
  fi
  WINHOME_U="$(wslpath -u "$WINHOME_W")"
  CARGO="$WINHOME_U/.cargo/bin/cargo.exe"
  CARGO_CONFIG_U="$WINHOME_U/.cargo/config.toml"
  MANIFEST="$(wslpath -w "$HERE/Cargo.toml")"
  TARGET_U="$WINHOME_U/.ogrep-target"
  export CARGO_TARGET_DIR="${WINHOME_W}\\.ogrep-target"
  export CARGO_INCREMENTAL=0
  export WSLENV="${WSLENV:+$WSLENV:}CARGO_TARGET_DIR:CARGO_INCREMENTAL:RUST_TEST_THREADS:RUST_BACKTRACE:CARGO_REGISTRIES_OGREP_MIRROR_TOKEN"
  BIN_DEFAULT="$WINHOME_U/bin"
  TASKLIST=/mnt/c/Windows/System32/tasklist.exe
  winpath() { wslpath -w "$1"; }
  KILL_HINT="taskkill.exe /IM ogrep.exe /F"
else
  CARGO=cargo
  CARGO_CONFIG_U="${CARGO_HOME:-$HOME/.cargo}/config.toml"
  # Hand cargo a real Windows path: cygpath turns //wsl.localhost/... into
  # \\wsl.localhost\..., which link.exe cannot mistake for an option.
  MANIFEST="$(cygpath -w "$HERE/Cargo.toml")"
  if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    TARGET_U="$(cygpath -u "$CARGO_TARGET_DIR")"
  else
    case "$HERE" in
      //*) # Repo on the WSL share: same Windows-local directory the WSL
           # branch uses, so the two hosts share one build cache.
           TARGET_U="$HOME/.ogrep-target"
           export CARGO_TARGET_DIR="$(cygpath -w "$TARGET_U")" ;;
      *)   TARGET_U="$HERE/target" ;;
    esac
  fi
  BIN_DEFAULT="$HOME/bin"
  TASKLIST=tasklist
  winpath() { cygpath -w "$1"; }
  KILL_HINT="taskkill //IM ogrep.exe //F"
fi
RELEASE_DIR="$TARGET_U/release"

if ! command -v "$CARGO" >/dev/null 2>&1 && [ ! -x "$CARGO" ]; then
  echo "x: no cargo at $CARGO. Install Rust for Windows (rustup, MSVC toolchain) first." >&2
  exit 1
fi

# Optional one-off registry: replace crates.io for this invocation only.
REGISTRY_ARGS=()
if [ -n "${OGREP_CARGO_REGISTRY:-}" ]; then
  REGISTRY_ARGS=(
    --config "registries.ogrep-mirror.index=\"$OGREP_CARGO_REGISTRY\""
    --config "source.ogrep-mirror.registry=\"$OGREP_CARGO_REGISTRY\""
    --config 'source.crates-io.replace-with="ogrep-mirror"'
  )
fi

# Insert --manifest-path right after the subcommand: appending it would land
# after any `--`, where it reaches the test binary instead of cargo.
run_cargo() {
  local sub="$1"; shift
  "$CARGO" ${REGISTRY_ARGS[@]+"${REGISTRY_ARGS[@]}"} "$sub" --manifest-path "$MANIFEST" "$@"
}

# Windows refuses to replace a running .exe, and `ogrep serve` runs for as
# long as you leave it. Say so before the copy fails with "Access is denied".
refuse_if_running() {
  command -v "$TASKLIST" >/dev/null 2>&1 || [ -x "$TASKLIST" ] || return 0
  local live
  # No /FI filter: Git Bash would rewrite /FI into a path.
  live="$("$TASKLIST" 2>/dev/null | grep -ci '^ogrep\.exe' || true)"
  [ "${live:-0}" -eq 0 ] && return 0
  echo "x: ogrep.exe is still running ($live process(es)), and Windows will not replace a running .exe." >&2
  echo "   Stop any 'ogrep serve' with Ctrl-C in its window, or end them all with:" >&2
  echo "     $KILL_HINT" >&2
  exit 1
}

case "${1:-help}" in
  fetch)   shift; run_cargo fetch --locked "$@"
           echo "every locked crate is available" ;;

  build)   shift; run_cargo build --locked -p ogrep-cli "$@"
           echo "binary: $TARGET_U/debug/ogrep.exe" ;;

  release) shift; run_cargo build --release --locked -p ogrep-cli "$@"
           echo "binary: $RELEASE_DIR/ogrep.exe" ;;

  install) shift
           dest="${1:-$BIN_DEFAULT}"
           refuse_if_running
           run_cargo build --release --locked -p ogrep-cli
           mkdir -p "$dest"
           cp -f "$RELEASE_DIR/ogrep.exe" "$dest/"
           echo "installed ogrep.exe to $dest"
           "$dest/ogrep.exe" --version
           if [ "$HOST" = win ] && [ "$dest" = "$BIN_DEFAULT" ]; then
             # Git Bash exports $HOME/bin:$PATH unconditionally (in
             # /etc/profile.d/env.sh), so there is only a stale command
             # cache to clear.
             echo "~/bin is already on Git Bash's PATH: run 'hash -r', then: ogrep --help"
           elif [ "$HOST" = wsl ]; then
             echo "from Git Bash: 'hash -r', then: ogrep --help"
             echo "(a Windows .exe; for a Linux binary in WSL use: make install)"
           else
             case ":$PATH:" in
               *":$dest:"*) echo "run 'hash -r', then: ogrep --help" ;;
               *) echo "note: $dest is not on PATH. Add:"
                  echo "        export PATH=\"$dest:\$PATH\"" ;;
             esac
           fi ;;

  test)    if [ "$#" -gt 0 ]; then shift; fi
           run_cargo test --workspace --locked "$@" ;;

  vendor)  shift
           run_cargo vendor --locked --versioned-dirs "$(winpath "$HERE/vendor")" "$@" >/dev/null
           echo "vendored every locked crate into ./vendor. To build from it offline, add to"
           echo "$(winpath "$HERE/.cargo/config.toml"):"
           echo ""
           echo '  [source.crates-io]'
           echo '  replace-with = "vendored-sources"'
           echo ''
           echo '  [source.vendored-sources]'
           echo '  directory = "vendor"' ;;

  doctor)  echo "host:          $HOST"
           echo "cargo:         $CARGO"
           echo "toolchain:     $("$CARGO" --version 2>&1) / $("${CARGO%cargo*}rustc${CARGO##*cargo}" -vV 2>/dev/null | sed -n 's/^host: //p')"
           echo "manifest:      $MANIFEST"
           echo "build output:  $TARGET_U"
           echo "install to:    $BIN_DEFAULT"
           if [ -n "${OGREP_CARGO_REGISTRY:-}" ]; then
             echo "registry:      $OGREP_CARGO_REGISTRY (from OGREP_CARGO_REGISTRY)"
           elif [ -f "$CARGO_CONFIG_U" ] && grep -q 'replace-with' "$CARGO_CONFIG_U"; then
             echo "registry:      crates.io replaced in $CARGO_CONFIG_U:"
             grep -n -A1 'replace-with\|^\[source\.' "$CARGO_CONFIG_U" | sed 's/^/                 /'
           else
             echo "registry:      crates.io (no replacement in $CARGO_CONFIG_U)"
           fi
           echo ""
           echo "checking that every locked crate resolves..."
           if run_cargo fetch --locked >/dev/null 2>&1; then
             echo "ok: all crates available"
           else
             echo "FAILED: cargo cannot fetch the locked crates. Output:" >&2
             run_cargo fetch --locked 2>&1 | tail -15 | sed 's/^/  /' >&2
             echo "" >&2
             echo "On a corporate network, point cargo at the Artifactory Cargo remote, in" >&2
             echo "$CARGO_CONFIG_U:" >&2
             echo "" >&2
             echo '  [registries.artifactory]' >&2
             echo '  index = "sparse+https://<host>/artifactory/api/cargo/<repo>/index/"' >&2
             echo '' >&2
             echo '  [source.artifactory]' >&2
             echo '  registry = "sparse+https://<host>/artifactory/api/cargo/<repo>/index/"' >&2
             echo '' >&2
             echo '  [source.crates-io]' >&2
             echo '  replace-with = "artifactory"' >&2
             echo "" >&2
             echo "or set OGREP_CARGO_REGISTRY for a single run (see the top of ./x)." >&2
             exit 1
           fi ;;

  help|-h|--help)
           sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//' ;;

  *)       sub="$1"; shift; run_cargo "$sub" "$@" ;;
esac
