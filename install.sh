#!/bin/sh
# Installs a prebuilt ogrep from its GitHub releases:
#
#   curl -fsSL https://raw.githubusercontent.com/eusthace811/ogrep/main/install.sh | sh
#
# Linux and macOS (x86_64, arm64) and Windows in Git Bash (x86_64). Settings:
#   OGREP_VERSION=v0.2.0        a release tag (default: the latest release)
#   OGREP_INSTALL_DIR=DIR       where to put it (default ~/.local/bin; ~/bin in Git Bash)
# Where downloaded executables are not allowed, build from source instead (see the README).
set -eu

REPO=eusthace811/ogrep
BIN=ogrep
VERSION="${OGREP_VERSION:-latest}"

say() { printf '%s\n' "$*" >&2; }
die() { say "install.sh: $*"; exit 1; }

case "$(uname -s)" in
  Linux)                OS=unknown-linux-musl ;;
  Darwin)               OS=apple-darwin ;;
  MINGW*|MSYS*|CYGWIN*) OS=pc-windows-msvc ;;
  *) die "no prebuilt release for $(uname -s); build from source (see the README)" ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  ARCH=x86_64 ;;
  arm64|aarch64) ARCH=aarch64 ;;
  *) die "no prebuilt release for $(uname -m); build from source (see the README)" ;;
esac
EXE=
EXT=tar.gz
DEFAULT_DIR="$HOME/.local/bin"
if [ "$OS" = pc-windows-msvc ]; then
  ARCH=x86_64 # Windows on Arm runs x86_64 executables.
  EXE=.exe
  EXT=zip
  DEFAULT_DIR="$HOME/bin" # Git Bash puts ~/bin on PATH.
fi
TARGET="$ARCH-$OS"
DIR="${OGREP_INSTALL_DIR:-$DEFAULT_DIR}"

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 -o "$2" "$1" || die "could not download $1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1" || die "could not download $1"; }
else
  die "needs curl or wget"
fi
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# install_release OWNER/REPO NAME TAG: downloads NAME-TARGET from that release, checks it
# against the release's SHA256SUMS and copies NAME into DIR.
install_release() {
  case "$3" in
    latest) url="https://github.com/$1/releases/latest/download" ;;
    *)      url="https://github.com/$1/releases/download/$3" ;;
  esac
  pkg="$2-$TARGET"
  say "downloading $pkg.$EXT from $1 ($3)"
  fetch "$url/$pkg.$EXT" "$TMP/$pkg.$EXT"
  fetch "$url/SHA256SUMS" "$TMP/SHA256SUMS"
  want="$(awk -v f="$pkg.$EXT" '$2 == f || $2 == "*" f { print $1 }' "$TMP/SHA256SUMS")"
  [ -n "$want" ] || die "$pkg.$EXT is not in the release's SHA256SUMS"
  [ "$(sha256 "$TMP/$pkg.$EXT")" = "$want" ] || die "$pkg.$EXT does not match its SHA256SUMS entry"
  if [ "$EXT" = zip ]; then
    powershell -NoProfile -Command "Expand-Archive -Force '$(cygpath -w "$TMP/$pkg.$EXT")' '$(cygpath -w "$TMP")'" ||
      die "could not unpack $pkg.zip"
  else
    tar -xzf "$TMP/$pkg.$EXT" -C "$TMP"
  fi
  mkdir -p "$DIR"
  cp -f "$TMP/$pkg/$2$EXE" "$DIR/$2$EXE" || die "could not write $DIR/$2$EXE (on Windows: is it running?)"
  chmod 755 "$DIR/$2$EXE"
  say "installed $("$DIR/$2$EXE" --version) to $DIR"
}

install_release "$REPO" "$BIN" "$VERSION"

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) say "note: $DIR is not on PATH. Add to your shell profile: export PATH=\"$DIR:\$PATH\"" ;;
esac
