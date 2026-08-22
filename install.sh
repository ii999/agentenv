#!/usr/bin/env bash
# Installs an agentenv release binary from GitHub Releases on macOS or Linux,
# together with the agentenv agent skill.
#
# Usage:
#   ./install.sh [--version <tag>] [--dir <install-dir>] [--claude-skills] [--no-skill]
#
# Options:
#   --version <tag>   Release tag to install, e.g. v0.1.1. Defaults to the
#                     latest release. AGENTENV_VERSION works the same way.
#   --dir <path>      Binary install directory. Defaults to ~/.local/bin.
#                     AGENTENV_INSTALL_DIR works the same way.
#   --claude-skills   Also install the agent skill to ~/.claude/skills for
#                     Claude Code, in addition to the ~/.agents/skills default.
#   --no-skill        Install the binary only.
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
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
}

version="${AGENTENV_VERSION:-}"
install_dir="${AGENTENV_INSTALL_DIR:-$HOME/.local/bin}"
install_skill=true
claude_skills=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version) [[ $# -ge 2 ]] || fail "--version needs a value"; version="$2"; shift 2 ;;
        --dir) [[ $# -ge 2 ]] || fail "--dir needs a value"; install_dir="$2"; shift 2 ;;
        --claude-skills) claude_skills=true; shift ;;
        --no-skill) install_skill=false; shift ;;
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
readonly extracted="$workdir/agentenv-${version}-${target}"

mkdir -p "$install_dir"
install -m 755 "$extracted/agentenv" "$install_dir/agentenv"
echo "Installed $("$install_dir/agentenv" --version) to $install_dir/agentenv"

# Replaces one skill directory under a skills root with the packaged copy.
install_skill_to() {
    local root="$1"
    local destination="$root/agentenv"
    if [[ -e "$destination" && ! -f "$destination/SKILL.md" ]]; then
        fail "$destination exists but is not an agentenv skill directory; move it aside and rerun"
    fi
    mkdir -p "$root"
    rm -rf "$destination"
    cp -R "$extracted/skills/agentenv" "$destination"
    echo "Installed the agentenv agent skill to $destination"
}

if [[ "$install_skill" == true ]]; then
    if [[ -f "$extracted/skills/agentenv/SKILL.md" ]]; then
        install_skill_to "$HOME/.agents/skills"
        if [[ "$claude_skills" == true ]]; then
            install_skill_to "$HOME/.claude/skills"
        fi
    else
        echo "install.sh: release $version ships no agent skill; skipping the skill install" >&2
    fi
fi

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "Add $install_dir to PATH to run 'agentenv' from any directory." ;;
esac
