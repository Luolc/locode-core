#!/usr/bin/env bash
#
# locode installer for macOS and Linux.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/luolc/locode-core/main/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/luolc/locode-core/main/install.sh | bash -s 0.1.6
#
# Installs the `locode` agent: run `locode` for the interactive TUI, or
# `locode -p "task"` for a headless one-shot.
#
# Env:
#   LOCODE_BIN_DIR   install destination (default: ~/.locode/bin)
#
# Re-running the same command updates in place: the binary is swapped
# atomically after a smoke test, and the shell-rc PATH block is
# marker-delimited and replaced, never duplicated.

set -euo pipefail

REPO="luolc/locode-core"
BIN_DIR="${LOCODE_BIN_DIR:-$HOME/.locode/bin}"

VERSION="${1:-}"
VERSION="${VERSION#v}"
if [ -n "$VERSION" ] && ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "Invalid version: $VERSION (expected X.Y.Z)" >&2
    exit 1
fi

if command -v curl >/dev/null 2>&1; then
    download() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
    download() { wget -q -O "$2" "$1"; }
else
    echo "Either curl or wget is required but neither is installed." >&2
    exit 1
fi

case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux)  os="unknown-linux-musl" ;;
    *)      echo "Unsupported OS: $(uname -s) (macOS and Linux only)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64|amd64)  arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)             echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
target="${arch}-${os}"

if [ -n "$VERSION" ]; then
    base="https://github.com/$REPO/releases/download/v$VERSION"
else
    base="https://github.com/$REPO/releases/latest/download"
fi
tarball="locode-$target.tar.gz"
checksum="locode-$target.sha256"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading locode (${VERSION:-latest}, $target)..." >&2
if ! download "$base/$tarball" "$tmp/$tarball"; then
    echo "Error: download failed: $base/$tarball" >&2
    echo "Either that release does not exist or it has no prebuilt binary for $target." >&2
    echo "Releases: https://github.com/$REPO/releases" >&2
    echo "Or build from source: cargo install --git https://github.com/Luolc/locode-core locode-app" >&2
    exit 1
fi
if ! download "$base/$checksum" "$tmp/$checksum"; then
    echo "Error: checksum download failed: $base/$checksum" >&2
    exit 1
fi

# The .sha256 file references the tarball by its asset name, so verify in $tmp.
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && sha256sum -c "$checksum" >/dev/null 2>&1) \
        || { echo "Error: checksum verification failed; nothing was installed." >&2; exit 1; }
elif command -v shasum >/dev/null 2>&1; then
    (cd "$tmp" && shasum -a 256 -c "$checksum" >/dev/null 2>&1) \
        || { echo "Error: checksum verification failed; nothing was installed." >&2; exit 1; }
else
    echo "Warning: no sha256 tool found; skipping checksum verification." >&2
fi

tar xzf "$tmp/$tarball" -C "$tmp"
chmod +x "$tmp/locode"
# Smoke-test before touching the destination: a bad download must never
# clobber a working install.
if ! "$tmp/locode" --version </dev/null >/dev/null 2>&1; then
    echo "Error: downloaded binary failed to run; nothing was installed." >&2
    exit 1
fi
installed="$("$tmp/locode" --version </dev/null)"

mkdir -p "$BIN_DIR"
mv -f "$tmp/locode" "$BIN_DIR/locode"
echo "Installed $installed to $BIN_DIR/locode" >&2

if ! command -v rg >/dev/null 2>&1; then
    echo "Note: ripgrep (rg) was not found on PATH; the search tools depend on it." >&2
    echo "Install it (e.g. 'brew install ripgrep' / 'apt install ripgrep') or set LOCODE_RG_PATH." >&2
fi

# --- Ensure locode is on PATH ---

path_has_dir() { case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac; }

# Symlink into a directory already on PATH so the command works immediately,
# without restarting the shell.
symlinked=""
if ! path_has_dir "$BIN_DIR"; then
    for candidate in "$HOME/.local/bin" "/usr/local/bin"; do
        if path_has_dir "$candidate" && [ -d "$candidate" ] && [ -w "$candidate" ]; then
            ln -sf "$BIN_DIR/locode" "$candidate/locode"
            symlinked="$candidate"
            echo "Symlinked $candidate/locode -> $BIN_DIR/locode" >&2
            break
        fi
    done
fi

# Also persist BIN_DIR on PATH for future sessions via a marker-delimited
# block in the shell rc file. On re-runs the block is replaced *in place*
# (position-stable) — never duplicated, never relocated to the end.
marker_open="# >>> locode installer >>>"
marker_close="# <<< locode installer <<<"
shell_name="$(basename "${SHELL:-}")"
rc_file=""
case "$shell_name" in
    bash) rc_file="$HOME/.bashrc" ;;
    zsh)  rc_file="$HOME/.zshrc" ;;
    fish) rc_file="$HOME/.config/fish/config.fish" ;;
esac

if [ -n "$rc_file" ]; then
    mkdir -p "$(dirname "$rc_file")"
    if [ "$shell_name" = "fish" ]; then
        block="fish_add_path $BIN_DIR"
    else
        block="export PATH=\"$BIN_DIR:\$PATH\""
    fi
    if [ -f "$rc_file" ] && grep -qs "$marker_open" "$rc_file"; then
        # Replace the body between the markers where it already sits, keeping
        # the block in its current position (so a re-run/update never moves it
        # to the end of the file). Malformed (open marker, no close) → the
        # tail is left intact; the block is rewritten from the open marker on.
        rc_tmp="$rc_file.tmp.$$"
        awk -v m1="$marker_open" -v m2="$marker_close" -v body="$block" '
            $0 == m1 { print; print body; skip=1; next }
            $0 == m2 { skip=0; print; next }
            skip     { next }
            { print }
        ' "$rc_file" > "$rc_tmp" && mv "$rc_tmp" "$rc_file"
        echo "Updated $BIN_DIR in PATH in $rc_file" >&2
    else
        printf '\n%s\n%s\n%s\n' "$marker_open" "$block" "$marker_close" >> "$rc_file"
        echo "Added $BIN_DIR to PATH in $rc_file" >&2
    fi
    # macOS login shells read .bash_profile, not .bashrc.
    if [ "$shell_name" = "bash" ] && [ "$(uname -s)" = "Darwin" ] \
        && [ -f "$HOME/.bash_profile" ] && ! grep -qs 'bashrc' "$HOME/.bash_profile"; then
        printf '\n[ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc"\n' >> "$HOME/.bash_profile"
    fi
fi

echo "" >&2
if path_has_dir "$BIN_DIR" || [ -n "$symlinked" ]; then
    echo "Done. Run 'locode --help' to get started." >&2
elif [ -n "$rc_file" ]; then
    echo "Done. Restart your terminal (or 'source $rc_file'), then run 'locode --help'." >&2
else
    echo "Done. Add $BIN_DIR to your PATH, then run 'locode --help':" >&2
    echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
fi
