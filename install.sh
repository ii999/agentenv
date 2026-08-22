#!/usr/bin/env bash
# Installs an agentenv release binary from GitHub Releases on macOS or Linux.
#
# Usage:
#   ./install.sh [--version <tag>] [--dir <install-dir>]
#
# Environment:
#   AGENTENV_VERSION      Release tag to install, e.g. v0.1.0 (same as --version).
#                         Defaults to the latest release.
#   AGENTENV_INSTALL_DIR  Install directory (same as --dir). Defaults to
#                         ~/.local/bin.
#
# Downloads use the GitHub CLI when it is installed and signed in, which is
# required while the repository is private; otherwise they use plain HTTPS.

set -euo pipefail

readonly repo="ii999/agentenv"

fail() {
    echo "install.sh: $*" >&2
    exit 1
}

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
}

version="${AGENTENV_VERSION:-}"
install_dir="${AGENTENV_INSTALL_DIR:-$HOME/.local/bin}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) [[ $# -ge 2 ]] || fail "--version needs a value"; version="$2"; shift 2 ;;
        --dir) [[ $# -ge 2 ]] || fail "--dir needs a value"; install_dir="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown option '$1'; run with --help for usage" ;;
    esac
done

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
    Darwin/arm64) target="aarch64-apple-darwin" ;;
    Darwin/x86_64) target="x86_64-apple-darwin" ;;
    Linux/x86_64) target="x86_64-unknown-linux-gnu" ;;
    *) fail "no prebuilt binary for $os/$arch; build from source with 'cargo build --release'" ;;
esac
readonly target

use_gh=false
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    use_gh=true
fi
readonly use_gh

if [[ -z "$version" ]]; then
    if [[ "$use_gh" == true ]]; then
        version="$(gh release view --repo "$repo" --json tagName --jq .tagName)"
    else
        version="$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
            | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')" || true
    fi
    [[ -n "$version" ]] || fail "cannot determine the latest release; sign in with \
'gh auth login' (required while the repository is private) or pass --version <tag>"
fi
readonly version
readonly asset="agentenv-${version}-${target}.tar.gz"

workdir="$(mktemp -d)"
readonly workdir
trap 'rm -rf "$workdir"' EXIT

download() {
    local name="$1"
    if [[ "$use_gh" == true ]]; then
        gh release download "$version" --repo "$repo" --pattern "$name" --dir "$workdir"
    else
        curl -fSL --output "$workdir/$name" \
            "https://github.com/$repo/releases/download/$version/$name" \
            || fail "cannot download $name from release $version; sign in with \
'gh auth login' (required while the repository is private)"
    fi
}

echo "Downloading agentenv $version for $target..."
download "$asset"
download "SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
    checksum=(sha256sum --check)
else
    checksum=(shasum -a 256 --check)
fi
(cd "$workdir" && grep -F "  $asset" SHA256SUMS | "${checksum[@]}" -) \
    || fail "checksum verification failed for $asset"

tar -xzf "$workdir/$asset" -C "$workdir"
mkdir -p "$install_dir"
install -m 755 "$workdir/agentenv-${version}-${target}/agentenv" "$install_dir/agentenv"

echo "Installed $("$install_dir/agentenv" --version) to $install_dir/agentenv"
case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "Add $install_dir to PATH to run 'agentenv' from any directory." ;;
esac
